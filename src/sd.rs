//! SD-card storage, the Rust counterpart of `sd.py`.
//!
//! The MicroPython code mounted the card with `os.mount` and read files with
//! the standard file API. Here we talk to the card over blocking SPI and parse
//! the FAT filesystem with `embedded-sdmmc`. The `VolumeManager` uses interior
//! mutability, so most methods only need `&self`; that lets the player borrow
//! the SD reader and the DAC independently.

use log::warn;
use embassy_rp::Peri;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{self, Blocking, MisoPin, MosiPin, ClkPin, Spi};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    Mode, RawDirectory, RawFile, RawVolume, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx,
    VolumeManager,
};

use core::str::from_utf8;

/// Bytes per stereo frame (16-bit L + 16-bit R), matching `FRAME_BYTES`.
pub const FRAME_BYTES: usize = 4;

/// SPI clock for the card. The Python driver streamed at 15 MHz; we use a
/// slightly more conservative 12 MHz. (Per spec, card *init* should happen at
/// <=400 kHz; most modern microSD cards tolerate a fast fixed clock, but if a
/// particular card refuses to mount, lower this value.)
const SD_BAUD: u32 = 12_000_000;

/// Largest number of tracks we will enumerate.
pub const MAX_TRACKS: usize = 64;

type SdSpiBus = Spi<'static, SPI0, Blocking>;
type SdSpiDevice = ExclusiveDevice<SdSpiBus, Output<'static>, Delay>;
type SdBlockDevice = SdCard<SdSpiDevice, Delay>;
type Vm = VolumeManager<SdBlockDevice, DummyTimeSource>;

pub const ALLOWED_EXTENSIONS: [&str; 1] = ["WAV"];

/// A trivial `TimeSource`; we do not set real file timestamps.
#[derive(Clone, Copy, Default)]
pub struct DummyTimeSource;

impl TimeSource for DummyTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

/// A fixed-capacity list of track filenames, the analogue of the sorted list
/// returned by `list_wav_files`.
pub struct TrackList {
    names: [ShortFileName; MAX_TRACKS],
    count: usize,
}

impl TrackList {
    pub fn new() -> Self {
        Self {
            names: core::array::from_fn(|_| ShortFileName::this_dir()),
            count: 0,
        }
    }

    /// Number of tracks discovered.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Borrow the name at `index`, if present.
    pub fn get(&self, index: usize) -> Option<&ShortFileName> {
        if index < self.count {
            Some(&self.names[index])
        } else {
            None
        }
    }

    fn push(&mut self, name: ShortFileName) {
        if self.count < MAX_TRACKS {
            self.names[self.count] = name;
            self.count += 1;
        }
    }
}

impl Default for TrackList {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the SD card, FAT volume manager, and the open root directory.
pub struct SdStorage {
    vm: Vm,
    volume: Option<RawVolume>,
    root: Option<RawDirectory>,
}

impl SdStorage {
    /// Build the SPI bus and card driver. Equivalent to constructing the
    /// `SDCard`/`SPI` objects in `sd.py`; the card is not yet mounted.
    ///
    /// Pins mirror `sd.py`: `clk` = SCK (GP2), `mosi` = MOSI (GP3),
    /// `miso` = MISO (GP0), `cs` = CS (GP1).
    pub fn new(
        spi0: Peri<'static, SPI0>,
        clk: Peri<'static, impl ClkPin<SPI0>>,
        mosi: Peri<'static, impl MosiPin<SPI0>>,
        miso: Peri<'static, impl MisoPin<SPI0>>,
        cs: Peri<'static, impl embassy_rp::gpio::Pin>,
    ) -> Self {
        let mut config = spi::Config::default();
        config.frequency = SD_BAUD;

        let bus = Spi::new_blocking(spi0, clk, mosi, miso, config);
        let cs = Output::new(cs, Level::High);
        let device = ExclusiveDevice::new(bus, cs, Delay).unwrap();
        let card = SdCard::new(device, Delay);
        let vm = VolumeManager::new(card, DummyTimeSource);

        Self {
            vm,
            volume: None,
            root: None,
        }
    }

    /// Mount the first FAT volume and open its root directory. Mirrors
    /// `mount_sd`. Returns `false` and logs on failure.
    pub fn mount(&mut self) -> bool {
        let volume = match self.vm.open_raw_volume(VolumeIdx(0)) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to open volume: {:?}", e);
                return false;
            }
        };
        let root = match self.vm.open_root_dir(volume) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to open root dir: {:?}", e);
                return false;
            }
        };
        self.volume = Some(volume);
        self.root = Some(root);
        true
    }

    /// Populate `out` with the allowed files in the root directory. Mirrors
    /// `list_files` (FAT enumeration order; not lexically sorted).
    pub fn list_files(&self, out: &mut TrackList) {
        out.count = 0;
        let Some(root) = self.root else {
            return;
        };
        let result = self.vm.iterate_dir(root, |entry| {
            if entry.attributes.is_directory() || entry.attributes.is_volume() {
                return;
            }
            // Skip macOS resource-fork sidecars (same filter as sd.py).
            if entry.name.base_name().starts_with(b"._") {
                return;
            }

            // Check if the file extension is allowed.
            match from_utf8(entry.name.extension()) {
                Ok(ext) => {
                    if ALLOWED_EXTENSIONS.contains(&ext) {
                        out.push(entry.name.clone());
                    }
                }
                Err(_) => {}
            }
        });
        
        if let Err(e) = result {
            warn!("Failed to list directory: {:?}", e);
        }
    }

    /// Open a file in the root directory for reading. Returns `None` on error.
    pub fn open_file(&self, name: &ShortFileName) -> Option<RawFile> {
        let root = self.root?;
        let file = match self.vm.open_file_in_dir(root, name, Mode::ReadOnly) {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open file: {:?}", e);
                return None;
            }
        };

        Some(file)
    }

    /// Close an open file handle.
    pub fn close(&self, file: RawFile) {
        let _ = self.vm.close_file(file);
    }

    /// Read up to `buf.len()` bytes. Returns bytes read (`0` at EOF or on error).
    pub fn read(&self, file: RawFile, buf: &mut [u8]) -> usize {
        match self.vm.read(file, buf) {
            Ok(n) => n,
            Err(e) => {
                warn!("SD read error: {:?}", e);
                0
            }
        }
    }

    /// Seek relative to the current file position.
    pub fn seek_from_current(&self, file: RawFile, offset: i32) -> bool {
        self.vm.file_seek_from_current(file, offset).is_ok()
    }

    /// Read exactly `buf.len()` bytes, returning `false` on short read/error.
    pub fn read_exact(&self, file: RawFile, buf: &mut [u8]) -> bool {
        let mut off = 0;
        while off < buf.len() {
            match self.vm.read(file, &mut buf[off..]) {
                Ok(0) => return false,
                Ok(n) => off += n,
                Err(_) => return false,
            }
        }
        true
    }
}
