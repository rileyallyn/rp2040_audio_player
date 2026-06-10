//! Loud square-wave test tone for DAC bring-up (counterpart of `dac_test.py`).
//!
//! Uses pure integer math (no `micromath`/float) and near full-scale amplitude
//! so it is unmistakable if any audio is reaching the speaker.

use log::info;
use static_cell::StaticCell;

use crate::dac::{pack_mono, Dac, SAMPLE_RATE, STREAM_BUF_FRAMES};

/// Near full-scale so it can't be missed.
const AMPLITUDE: i16 = 30_000;

const BUF_FRAMES: usize = STREAM_BUF_FRAMES;

struct ToneBuffers {
    a: [u32; BUF_FRAMES],
    b: [u32; BUF_FRAMES],
}

static TONE_BUFFERS: StaticCell<ToneBuffers> = StaticCell::new();

/// Stream a square-wave tone for `seconds`, then mute.
pub async fn play(dac: &mut Dac, freq_hz: u32, seconds: u32) {
    if seconds == 0 || freq_hz == 0 {
        return;
    }

    info!("Playing {} Hz square tone ({}s, loud)", freq_hz, seconds);

    let bufs = TONE_BUFFERS.init_with(|| ToneBuffers {
        a: [0; BUF_FRAMES],
        b: [0; BUF_FRAMES],
    });

    // Samples per half-period of the square wave.
    let half_period = (SAMPLE_RATE / freq_hz / 2).max(1) as u64;
    let total_frames = (SAMPLE_RATE as u64) * (seconds as u64);
    let mut frame = 0u64;

    // Clock I2S and unmute before audible samples.
    dac.start_output().await;

    dac.stream(&mut bufs.a, &mut bufs.b, |out| {
        if frame >= total_frames {
            return false;
        }
        let frames = out.len();
        for i in 0..frames {
            let s = if frame >= total_frames {
                0
            } else if (frame / half_period) % 2 == 0 {
                AMPLITUDE
            } else {
                -AMPLITUDE
            };
            out[i] = pack_mono(s);
            if frame < total_frames {
                frame += 1;
            }
        }
        frame < total_frames
    })
    .await;

    dac.mute();
    info!("Test tone finished");
}
