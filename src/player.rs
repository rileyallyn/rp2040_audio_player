//! Playback controller, the Rust port of `player/PlaybackController.py`.
//!
//! The Python version was a *polled* design: a `pump()` call repeatedly filled
//! one half of a ping-pong buffer from the SD card while the other half drained
//! into a non-blocking I2S ring. Embassy gives us the same double-buffering for
//! free with async DMA: we start a transfer of the "front" buffer, refill the
//! "back" buffer from the card while that DMA runs, then `await` and swap. This
//! is gapless and keeps SD reads overlapped with audio output.
//!
//! Transport control (the Python `pause`/`resume`/`stop`/`toggle_pause`) maps to
//! [`Command`] values delivered through a [`PlayerControl`] signal, which lets a
//! separate task (buttons, UI, etc.) steer playback while [`PlaybackController::play`]
//! is awaiting DMA.

use log::{info, warn};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embedded_sdmmc::ShortFileName;
use static_cell::StaticCell;

use crate::dac::Dac;
use crate::display::Display;
use crate::sd::{FRAME_BYTES, SdStorage, TrackList};

/// Stereo frames per buffer half. At 44.1 kHz this is ~46 ms of audio.
pub const BUFFER_FRAMES: usize = 2048;

const BUFFER_WORDS: usize = BUFFER_FRAMES;

/// Silence frames streamed while the SD card is scanned.
const WARMUP_WORDS: usize = 512;

struct PlayBuffers {
    a: [u32; BUFFER_WORDS],
    b: [u32; BUFFER_WORDS],
    staging: [u8; BUFFER_FRAMES * FRAME_BYTES],
}

static PLAY_BUFFERS: StaticCell<PlayBuffers> = StaticCell::new();

/// Transport states, mirroring `PlaybackController.STATES`.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

/// Transport commands. Map 1:1 to the Python control methods:
/// `pause()` -> [`Command::Pause`], `resume()` -> [`Command::Resume`],
/// `stop()` -> [`Command::Stop`], `toggle_pause()` -> [`Command::Toggle`].
#[allow(dead_code)] // public transport API, sent by external control tasks
#[derive(Clone, Copy, defmt::Format)]
pub enum Command {
    Pause,
    Resume,
    Stop,
    Toggle,
}

/// Channel used to deliver [`Command`]s to a running [`PlaybackController::play`].
/// Construct one as a `static` (e.g. `static CONTROL: PlayerControl = PlayerControl::new();`)
/// so both the player task and a controller task can reach it.
pub type PlayerControl = Signal<CriticalSectionRawMutex, Command>;

/// Streams WAV files from the SD card to the DAC.
pub struct PlaybackController {
    dac: Dac,
    sd: SdStorage,
    state: State,
    buffers: &'static mut PlayBuffers,
}

#[allow(dead_code)] // shutdown/state/is_playing mirror the Python public API
impl PlaybackController {
    /// Create a controller from an initialized DAC and SD storage.
    pub fn new(dac: Dac, sd: SdStorage) -> Self {
        let buffers = PLAY_BUFFERS.init_with(|| PlayBuffers {
            a: [0; BUFFER_WORDS],
            b: [0; BUFFER_WORDS],
            staging: [0; BUFFER_FRAMES * FRAME_BYTES],
        });
        Self {
            dac,
            sd,
            state: State::Stopped,
            buffers,
        }
    }

    /// Stream a few silence buffers so I2S clocks are running before track
    /// enumeration or playback. Output stays muted.
    pub async fn warm_up_i2s(&mut self) {
        let silence = [0u32; WARMUP_WORDS];
        for _ in 0..4 {
            self.dac.drain(&silence).await;
        }
        info!("I2S clocks primed (muted)");
    }

    /// Mount the SD card. The DAC is constructed already muted, so there is no
    /// separate init step for it. Mirrors `initialize()`. Returns `false` if the
    /// card could not be mounted.
    pub fn initialize(&mut self) -> bool {
        self.dac.mute();
        self.sd.mount()
    }

    /// Stop playback and leave the DAC muted. Mirrors `shutdown()`.
    pub fn shutdown(&mut self) {
        self.dac.mute();
        self.state = State::Stopped;
    }

    /// Enumerate `.wav` tracks on the card into `out`. Mirrors `list_tracks()`.
    pub fn list_tracks(&self, out: &mut TrackList) {
        self.sd.list_wav_files(out);
    }

    /// Current transport state.
    pub fn state(&self) -> State {
        self.state
    }

    /// True while a track is actively playing. Mirrors `is_playing()`.
    pub fn is_playing(&self) -> bool {
        self.state == State::Playing
    }

    /// Stream `name` to completion, honoring transport commands from `control`.
    ///
    /// `index`/`total` and `display` are only used to render the on-device
    /// "now playing" screen as the track starts, pauses, resumes, and stops.
    ///
    /// Returns `true` when the track played out fully, `false` if it was stopped
    /// early or could not be opened. This is the async fusion of the Python
    /// `play()` + `pump()`/`_fill()`/`_drain()`/`_maybe_finish()` loop.
    pub async fn play(
        &mut self,
        name: &ShortFileName,
        index: usize,
        total: usize,
        control: &PlayerControl,
        display: Option<&mut Display>,
    ) -> bool {
        // Split borrows so the SD reader and DAC can be used simultaneously.
        let Self { dac, sd, state, buffers } = self;
        let mut display = display;

        let file = match sd.open_wav(name) {
            Some(f) => f,
            None => {
                warn!("Failed to open track");
                *state = State::Stopped;
                if let Some(d) = display.as_deref_mut() {
                    d.message("Open failed", "skipping track");
                }
                return false;
            }
        };

        let mut front: &mut [u32] = &mut buffers.a;
        let mut back: &mut [u32] = &mut buffers.b;
        let staging = &mut buffers.staging;

        dac.mute();
        *state = State::Playing;
        if let Some(d) = display.as_deref_mut() {
            d.now_playing(index, total, name, *state);
        }

        // Start I2S clocks and unmute before PCM (`dac_test.py` / `test_playback.py`).
        dac.start_output().await;
        let mut unmuted = true;

        // Prime the first buffer; bail out if the file holds no audio.
        let primed = sd.fill_words(file, front, staging);
        if primed == 0 {
            warn!("Track contained no PCM samples");
            sd.close(file);
            dac.mute();
            *state = State::Stopped;
            return false;
        }
        info!("Streaming {} frame(s) from track", primed);

        let finished;
        'outer: loop {
            // Service any pending transport command before queueing more audio.
            if let Some(cmd) = control.try_take() {
                match cmd {
                    Command::Stop => {
                        finished = false;
                        break 'outer;
                    }
                    Command::Pause | Command::Toggle => {
                        *state = State::Paused;
                        dac.mute();
                        if let Some(d) = display.as_deref_mut() {
                            d.now_playing(index, total, name, *state);
                        }
                        // Park here until resumed/stopped. The I2S FIFO drains
                        // and the PIO state machine simply stalls, output muted.
                        loop {
                            match control.wait().await {
                                Command::Resume | Command::Toggle => {
                                    *state = State::Playing;
                                    if unmuted {
                                        dac.unmute();
                                    }
                                    if let Some(d) = display.as_deref_mut() {
                                        d.now_playing(index, total, name, *state);
                                    }
                                    break;
                                }
                                Command::Stop => {
                                    finished = false;
                                    break 'outer;
                                }
                                Command::Pause => {}
                            }
                        }
                    }
                    Command::Resume => {}
                }
            }

            // Start DMA on the full front buffer, refill back while it plays,
            // then wait for the transfer to finish and swap halves.
            let transfer = dac.write(front);
            let next = sd.fill_words(file, back, staging);
            transfer.await;

            // Fade in after the first buffer has been delivered, like the
            // `_i2s_bytes_out >= buffer_size` unmute in the Python drain loop.
            if !unmuted {
                dac.unmute();
                unmuted = true;
                info!("DAC unmuted");
            }

            core::mem::swap(&mut front, &mut back);

            // `back` carried no fresh audio: end of file, nothing left to play.
            if next == 0 {
                finished = true;
                break 'outer;
            }
        }

        dac.mute();
        sd.close(file);
        *state = State::Stopped;
        if let Some(d) = display.as_deref_mut() {
            d.now_playing(index, total, name, *state);
        }
        if finished {
            info!("Track finished");
        }
        finished
    }
}
