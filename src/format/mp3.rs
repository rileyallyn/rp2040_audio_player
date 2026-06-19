use embedded_sdmmc::{RawFile, ShortFileName};
use log::{info, warn};
use nanomp3::{Channels, Decoder, FrameInfo, MAX_SAMPLES_PER_FRAME};

use crate::dac::{pack_frame, SAMPLE_RATE};
use crate::sd::SdStorage;

/// minimp3 recommends ≥16 KiB of compressed data in the decode window.
const COMPACT_THRESHOLD: usize = 4096;

pub struct Mp3Decoder {
    file: RawFile,
    decoder: Decoder,
    pcm_buf: [f32; MAX_SAMPLES_PER_FRAME],
    /// Index into `pcm_buf` (interleaved f32 samples).
    pending_off: usize,
    pending_len: usize,
    channels: Channels,
    sample_rate: u32,
    /// Start of unconsumed compressed data in the shared staging buffer.
    staging_off: usize,
    /// Bytes of compressed audio from `staging_off`.
    staging_len: usize,
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
            staging_off: 0,
            staging_len: 0,
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

            if self.decode_next_frame(sd, staging) {
                continue;
            }
            if self.eof {
                break;
            }
            // Partial MPEG frame buffered in staging; retry without zero-padding.
            if self.staging_len == 0 {
                break;
            }
        }

        for w in words.iter_mut().skip(out) {
            *w = 0;
        }
        if out < target && !self.eof {
            warn!("MP3 partial buffer: {} / {} frames", out, target);
        }
        out
    }

    pub fn close(self, sd: &SdStorage) {
        sd.close(self.file);
    }

    fn drain_pending(&mut self, words: &mut [u32], out: usize, target: usize) -> usize {
        let mono = self.channels == Channels::Mono;
        let mut written = 0;
        while out + written < target && self.pending_off < self.pending_len {
            let left = f32_to_i16(self.pcm_buf[self.pending_off]);
            let right = if mono {
                left
            } else {
                f32_to_i16(self.pcm_buf[self.pending_off + 1])
            };
            self.pending_off += if mono { 1 } else { 2 };
            words[out + written] = pack_frame(left, right);
            written += 1;
        }
        written
    }

    fn compact_if_needed(&mut self, staging: &mut [u8]) {
        if self.staging_off == 0 {
            return;
        }
        if self.staging_off >= COMPACT_THRESHOLD
            || self.staging_off + self.staging_len >= staging.len()
        {
            staging.copy_within(self.staging_off..self.staging_off + self.staging_len, 0);
            self.staging_off = 0;
        }
    }

    fn read_into_staging(&mut self, sd: &SdStorage, staging: &mut [u8]) {
        if self.eof {
            return;
        }
        while self.staging_off + self.staging_len < staging.len() {
            let write_at = self.staging_off + self.staging_len;
            let n = sd.read(self.file, &mut staging[write_at..]);
            if n == 0 {
                self.eof = true;
                break;
            }
            self.staging_len += n;
        }
    }

    /// Pull the next MPEG frame from SD. Returns `false` when no more audio is available.
    fn decode_next_frame(&mut self, sd: &SdStorage, staging: &mut [u8]) -> bool {
        loop {
            self.compact_if_needed(staging);
            self.read_into_staging(sd, staging);

            if self.staging_len == 0 {
                return false;
            }

            let end = self.staging_off + self.staging_len;
            let src = &staging[self.staging_off..end];
            let (consumed, info) = self.decoder.decode(src, &mut self.pcm_buf);

            if consumed == 0 {
                if self.staging_len >= staging.len() {
                    warn!("MP3 sync lost");
                    self.eof = true;
                    return false;
                }
                if self.eof {
                    return false;
                }
                continue;
            }

            self.staging_off += consumed;
            self.staging_len -= consumed;

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

    fn note_format(&mut self, info: &FrameInfo) -> bool {
        if self.sample_rate == 0 {
            self.sample_rate = info.sample_rate;
            self.channels = info.channels;
            info!(
                "MP3 stream: {} Hz, {} kb/s",
                self.sample_rate, info.bitrate
            );
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
