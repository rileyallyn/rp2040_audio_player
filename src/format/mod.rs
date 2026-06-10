mod wav;

use embedded_sdmmc::ShortFileName;

use crate::sd::SdStorage;
use wav::WavDecoder;

pub enum AudioSource {
    Wav(WavDecoder),
}

impl AudioSource {
    pub fn open(sd: &SdStorage, name: &ShortFileName) -> Option<Self> {
        if name.extension().eq_ignore_ascii_case(b"WAV") {
            WavDecoder::open(sd, name).map(Self::Wav)
        } else {
            None
        }
    }

    pub fn fill_frames(
        &self,
        sd: &SdStorage,
        words: &mut [u32],
        staging: &mut [u8],
    ) -> usize {
        match self {
            Self::Wav(decoder) => decoder.fill_frames(sd, words, staging),
        }
    }

    pub fn close(self, sd: &SdStorage) {
        match self {
            Self::Wav(decoder) => decoder.close(sd),
        }
    }
}
