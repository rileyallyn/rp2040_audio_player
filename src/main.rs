#![no_std]
#![no_main]

mod dac;
mod display;
mod format;
mod input;
mod player;
mod sd;
mod tone;
mod ui;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::{peripherals::USB, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Instant, Timer};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use dac::Dac;
use display::Display;
use log::info;
use player::{
    PlaybackController, PlaybackDoneSignal, PlayRequestSignal, PlayerControl,
};
use sd::{SdStorage, TrackList};
use ui::UiApp;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});

type PlayerMutex = Mutex<CriticalSectionRawMutex, PlaybackController>;

static CONTROL: PlayerControl = PlayerControl::new();
static PLAY_REQUEST: PlayRequestSignal = PlayRequestSignal::new();
static PLAYBACK_DONE: PlaybackDoneSignal = PlaybackDoneSignal::new();
static PLAYER: StaticCell<PlayerMutex> = StaticCell::new();

/// 440 Hz sine at boot for DAC bring-up (`test_playback.py`). Set to 0 to skip.
const BOOT_TONE_SECS: u32 = 3;

#[embassy_executor::task]
async fn logger_task(usb: embassy_rp::Peri<'static, embassy_rp::peripherals::USB>) {
    let driver = embassy_rp::usb::Driver::new(usb, Irqs);
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::task]
async fn heartbeat_task() {
    loop {
        Timer::after_secs(5).await;
        info!("heartbeat: uptime {}s", Instant::now().as_secs());
    }
}

#[embassy_executor::task]
async fn playback_task(player: &'static PlayerMutex) -> ! {
    loop {
        let req = PLAY_REQUEST.wait().await;
        let finished = {
            let mut player = player.lock().await;
            player
                .play(&req.name, req.index, req.total, &CONTROL, None)
                .await
        };
        info!("Playback ended (finished={})", finished);
        PLAYBACK_DONE.signal(());
    }
}

#[embassy_executor::task]
async fn ui_task(player: &'static PlayerMutex, mut display: Option<Display>) -> ! {
    let tracks = {
        let player = player.lock().await;
        let mut tracks = TrackList::new();
        player.list_tracks(&mut tracks);
        tracks
    };
    info!("Found {} track(s)", tracks.len());

    let mut app = UiApp::new(tracks);
    app.run(
        display.as_mut(),
        player,
        &CONTROL,
        &PLAY_REQUEST,
        &PLAYBACK_DONE,
    )
    .await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    let p = embassy_rp::init(Default::default());

    spawner.must_spawn(logger_task(p.USB));

    Timer::after_secs(3).await;
    info!("MP3 Player booted; USB serial logging active");
    spawner.must_spawn(heartbeat_task());

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

    let sd = SdStorage::new(p.SPI0, p.PIN_2, p.PIN_3, p.PIN_0, p.PIN_1);

    let mut display = Display::new(p.I2C0, p.PIN_13, p.PIN_12);
    if display.is_none() {
        info!("OLED not detected; running without display");
    }
    if let Some(d) = display.as_mut() {
        d.splash();
    }

    let player = PlaybackController::new(dac, sd);
    let player = PLAYER.init(Mutex::new(player));

    {
        let mut player = player.lock().await;
        while !player.initialize() {
            info!("SD card not mounted; retrying...");
            if let Some(d) = display.as_mut() {
                d.message("SD mount failed", "retrying...");
            }
            Timer::after_secs(2).await;
        }
        player.warm_up_i2s().await;
    }

    input::spawn(
        &spawner,
        p.PIN_20.into(),
        p.PIN_21.into(),
        p.PIN_22.into(),
    );

    spawner.must_spawn(playback_task(player));
    spawner.must_spawn(ui_task(player, display));

    loop {
        Timer::after_secs(3600).await;
    }
}
