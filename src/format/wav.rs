use embedded_sdmmc::{RawFile, ShortFileName};
use log::warn;

use crate::dac::pack_frame;
use crate::sd::{SdStorage, FRAME_BYTES};

pub struct WavDecoder {
    file: RawFile,
}

impl WavDecoder {
    pub fn open(sd: &SdStorage, name: &ShortFileName) -> Option<Self> {
        let file = sd.open_file(name)?;
        let decoder = Self { file};
        if !decoder.validate_header(sd) {
            sd.close(file);
            return None;
        }
        if !decoder.validate_chunk(sd) {
            sd.close(file);
            return None;
        }
        Some(decoder)
    }

    /// Fill `words` with packed stereo frames, zero-padding any remainder.
    /// Returns the count of real frames (0 at EOF).
    pub fn fill_frames(
        &mut self,
        sd: &SdStorage,
        words: &mut [u32],
        staging: &mut [u8],
    ) -> usize {
        let frames_cap = words.len();
        let need = frames_cap * FRAME_BYTES;
        let buf = &mut staging[..need];

        let mut got = 0;
        while got < need {
            let n = sd.read(self.file, &mut buf[got..]);
            if n == 0 {
                break;
            }
            got += n;
        }

        let frames = got / FRAME_BYTES;
        for i in 0..frames {
            let b = i * FRAME_BYTES;
            let left = i16::from_le_bytes([buf[b], buf[b + 1]]);
            let right = i16::from_le_bytes([buf[b + 2], buf[b + 3]]);
            words[i] = pack_frame(left, right);
        }
        for w in words.iter_mut().skip(frames) {
            *w = 0;
        }
        frames
    }

    pub fn close(self, sd: &SdStorage) {
        sd.close(self.file);
    }

    fn validate_header(&self, sd: &SdStorage) -> bool {
        let mut header = [0u8; 12];
        if !sd.read_exact(self.file, &mut header) {
            return false;
        }
        if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
            warn!("Not a WAV file");
            return false;
        }
        true
    }

    fn validate_chunk(&self, sd: &SdStorage) -> bool {
        loop {
            let mut chunk = [0u8; 8];
            if !sd.read_exact(self.file, &mut chunk) {
                return false;
            }
            let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            if &chunk[0..4] == b"data" {
                return true;
            }
            let skip = size as i32 + (size & 1) as i32;
            if !sd.seek_from_current(self.file, skip) {
                return false;
            }
        }
    }
}
