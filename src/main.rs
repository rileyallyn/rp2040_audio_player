#![no_std]
#![no_main]

mod dac;
mod display;
mod player;
mod sd;
mod tone;
mod format;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::{peripherals::USB, usb};

use embassy_time::{Instant, Timer};
use {defmt_rtt as _, panic_probe as _};

use dac::Dac;
use display::Display;
use log::info;
use player::{PlaybackController, PlayerControl};
use sd::{SdStorage, TrackList};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});



/// Transport control channel. Signal commands here (e.g. from a button task)
/// to pause/resume/stop the track currently being played.
static CONTROL: PlayerControl = PlayerControl::new();

/// 440 Hz sine at boot for DAC bring-up (`test_playback.py`). Set to 0 to skip.
const BOOT_TONE_SECS: u32 = 3;

#[embassy_executor::task]
async fn logger_task(usb: embassy_rp::Peri<'static, embassy_rp::peripherals::USB>) {
    let driver = embassy_rp::usb::Driver::new(usb, Irqs);

    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

/// Periodically logs uptime so you can confirm the USB serial link is alive
/// even when nothing else is happening.
#[embassy_executor::task]
async fn heartbeat_task() {
    loop {
        Timer::after_secs(5).await;
        info!("heartbeat: uptime {}s", Instant::now().as_secs());
    }
}


#[embassy_executor::main]
async fn main(_spawner: Spawner) -> ! {
    let p = embassy_rp::init(Default::default());

    _spawner.must_spawn(logger_task(p.USB));

    // Give USB time to enumerate and the host to open the serial port before the
    // important startup logs print (anything logged before this is likely lost).
    Timer::after_secs(3).await;
    info!("MP3 Player booted; USB serial logging active");
    _spawner.must_spawn(heartbeat_task());

    // DAC: PIO0-driven I2S into a PCM5102A.
    // DIN=GP14, BCK=GP15, LRCK=GP16, XSMT=GP6, MCLK=GP17 (PWM).
    let pio = Pio::new(p.PIO0, Irqs);
    let mut dac = Dac::new(
        pio,
        p.DMA_CH0,
        p.PIN_14,
        p.PIN_15,
        p.PIN_16,
        p.PIN_6,
        p.PWM_SLICE0,
        p.PIN_17,
        Irqs,
    );

    if BOOT_TONE_SECS > 0 {
        tone::play(&mut dac, 440, BOOT_TONE_SECS).await;
    }

    // SD card on SPI0: SCK=GP2, MOSI=GP3, MISO=GP0, CS=GP1.
    let sd = SdStorage::new(p.SPI0, p.PIN_2, p.PIN_3, p.PIN_0, p.PIN_1);

    // Status OLED on I2C0: SCL=GP13, SDA=GP12 (optional; runs headless if absent).
    let mut display = Display::new(p.I2C0, p.PIN_13, p.PIN_12);
    if display.is_none() {
        info!("OLED not detected; running without display");
    }
    if let Some(d) = display.as_mut() {
        d.splash();
    }

    let mut player = PlaybackController::new(dac, sd);

    while !player.initialize() {
        info!("SD card not mounted; retrying...");
        if let Some(d) = display.as_mut() {
            d.message("SD mount failed", "retrying...");
        }
        Timer::after_secs(2).await;
    }

    // Start I2S clocks (muted) before SD directory enumeration.
    player.warm_up_i2s().await;

    let mut tracks = TrackList::new();
    player.list_tracks(&mut tracks);
    info!("Found {} track(s)", tracks.len());

    loop {
        if tracks.is_empty() {
            info!("No .wav files found on SD card");
            if let Some(d) = display.as_mut() {
                d.message("No tracks", "insert SD card");
            }
            Timer::after_secs(2).await;
            player.list_tracks(&mut tracks);
            continue;
        }

        let total = tracks.len();
        for i in 0..total {
            if let Some(name) = tracks.get(i).cloned() {
                info!("Playing track {}", i);
                player
                    .play(&name, i, total, &CONTROL, display.as_mut())
                    .await;
            }
        }
    }
}
