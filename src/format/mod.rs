mod mp3;
mod wav;

use embedded_sdmmc::ShortFileName;

use crate::sd::SdStorage;
use mp3::Mp3Decoder;
use wav::WavDecoder;

pub enum AudioSource {
    Wav(WavDecoder),
    Mp3(Mp3Decoder),
}

impl AudioSource {
    pub fn open(sd: &SdStorage, name: &ShortFileName) -> Option<Self> {
        if name.extension().eq_ignore_ascii_case(b"WAV") {
            WavDecoder::open(sd, name).map(Self::Wav)
        } else if name.extension().eq_ignore_ascii_case(b"MP3") {
            Mp3Decoder::open(sd, name).map(Self::Mp3)
        } else {
            None
        }
    }

    pub fn fill_frames(
        &mut self,
        sd: &SdStorage,
        words: &mut [u32],
        staging: &mut [u8],
    ) -> usize {
        match self {
            Self::Wav(decoder) => decoder.fill_frames(sd, words, staging),
            Self::Mp3(decoder) => decoder.fill_frames(sd, words, staging),
        }
    }

    pub fn close(self, sd: &SdStorage) {
        match self {
            Self::Wav(decoder) => decoder.close(sd),
            Self::Mp3(decoder) => decoder.close(sd),
        }
    }
}
