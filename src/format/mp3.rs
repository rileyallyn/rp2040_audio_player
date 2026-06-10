use embedded_sdmmc::{RawFile, ShortFileName};
use log::warn;
use nanomp3::{Channels, Decoder, FrameInfo, MAX_SAMPLES_PER_FRAME};

use crate::dac::{pack_frame, SAMPLE_RATE};
use crate::sd::SdStorage;

/// Room for one max-sized MPEG frame plus a little slack.
const MP3_CARRY_CAP: usize = 1536;

pub struct Mp3Decoder {
    file: RawFile,
    decoder: Decoder,
    pcm_buf: [f32; MAX_SAMPLES_PER_FRAME],
    /// Index into `pcm_buf` (interleaved f32 samples).
    pending_off: usize,
    pending_len: usize,
    channels: Channels,
    sample_rate: u32,
    mp3_carry: [u8; MP3_CARRY_CAP],
    mp3_carry_len: usize,
    eof: bool,
}

impl Mp3Decoder {
    pub fn open(sd: &SdStorage, name: &ShortFileName) -> Option<Self> {
        let file = sd.open_file(name)?;
        Some(Self {
            file,
            decoder: Decoder::new(),
            pcm_buf: [0.0; MAX_SAMPLES_PER_FRAME],
            pending_off: 0,
            pending_len: 0,
            channels: Channels::Stereo,
            sample_rate: 0,
            mp3_carry: [0; MP3_CARRY_CAP],
            mp3_carry_len: 0,
            eof: false,
        })
    }

    /// Decode MP3 from SD into packed stereo I2S frames.
    /// Returns the count of real frames (0 at EOF).
    pub fn fill_frames(
        &mut self,
        sd: &SdStorage,
        words: &mut [u32],
        staging: &mut [u8],
    ) -> usize {
        let target = words.len();
        let mut out = 0;

        while out < target {
            if self.pending_off < self.pending_len {
                out += self.drain_pending(words, out, target);
                continue;
            }
            self.pending_off = 0;
            self.pending_len = 0;

            if self.eof {
                break;
            }

            if !self.decode_next_frame(sd, staging) {
                break;
            }
        }

        for w in words.iter_mut().skip(out) {
            *w = 0;
        }
        out
    }

    pub fn close(self, sd: &SdStorage) {
        sd.close(self.file);
    }

    fn drain_pending(&mut self, words: &mut [u32], out: usize, target: usize) -> usize {
        let mut written = 0;
        while out + written < target && self.pending_off < self.pending_len {
            let left = f32_to_i16(self.pcm_buf[self.pending_off]);
            let right = if self.channels == Channels::Mono {
                left
            } else {
                f32_to_i16(self.pcm_buf[self.pending_off + 1])
            };
            self.pending_off += if self.channels == Channels::Mono { 1 } else { 2 };
            words[out + written] = pack_frame(left, right);
            written += 1;
        }
        written
    }

    /// Pull the next MPEG frame from SD. Returns `false` when no more audio is available.
    fn decode_next_frame(&mut self, sd: &SdStorage, staging: &mut [u8]) -> bool {
        loop {
            let mut filled = self.mp3_carry_len;
            if filled > 0 {
                staging[..filled].copy_from_slice(&self.mp3_carry[..filled]);
                self.mp3_carry_len = 0;
            }

            if filled < staging.len() {
                let n = sd.read(self.file, &mut staging[filled..]);
                if n == 0 && filled == 0 {
                    self.eof = true;
                    return false;
                }
                filled += n;
            }

            let src = &staging[..filled];
            let (consumed, info) = self.decoder.decode(src, &mut self.pcm_buf);

            if consumed == 0 {
                if filled > MP3_CARRY_CAP {
                    warn!("MP3 sync lost");
                    self.eof = true;
                    return false;
                }
                self.mp3_carry[..filled].copy_from_slice(src);
                self.mp3_carry_len = filled;
                return false;
            }

            self.save_mp3_tail(staging, filled, consumed);

            let Some(info) = info else {
                // ID3 or other non-audio frame; keep decoding from the refreshed buffer.
                continue;
            };

            if !self.note_format(&info) {
                self.eof = true;
                return false;
            }

            self.pending_off = 0;
            self.pending_len =
                info.samples_produced * info.channels.num() as usize;
            return self.pending_len > 0;
        }
    }

    fn save_mp3_tail(&mut self, staging: &[u8], filled: usize, consumed: usize) {
        if consumed >= filled {
            return;
        }
        let tail = filled - consumed;
        if tail > MP3_CARRY_CAP {
            warn!("MP3 carry overflow");
            self.eof = true;
            return;
        }
        self.mp3_carry[..tail].copy_from_slice(&staging[consumed..filled]);
        self.mp3_carry_len = tail;
    }

    fn note_format(&mut self, info: &FrameInfo) -> bool {
        if self.sample_rate == 0 {
            self.sample_rate = info.sample_rate;
            self.channels = info.channels;
            if self.sample_rate != SAMPLE_RATE {
                warn!(
                    "Unsupported MP3 sample rate {} Hz (need {})",
                    self.sample_rate, SAMPLE_RATE
                );
                return false;
            }
        }
        true
    }
}

#[inline]
fn f32_to_i16(sample: f32) -> i16 {
    if sample >= 1.0 {
        i16::MAX
    } else if sample <= -1.0 {
        i16::MIN
    } else {
        (sample * 32768.0) as i16
    }
}
