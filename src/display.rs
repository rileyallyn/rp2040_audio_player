//! SSD1306 128x64 OLED status display over I2C, the Rust counterpart of the
//! MicroPython `ssd1306` / `oled_test.py` / `ui` code.
//!
//! Since the board has no debug probe attached for day-to-day use, this gives
//! on-device feedback: boot status, SD mount result, track count, and a live
//! "now playing" screen that reflects play/pause/stop transitions.

use core::fmt::Write;

use embassy_rp::Peri;
use embassy_rp::i2c::{self, Blocking, I2c, SclPin, SdaPin};
use embassy_rp::peripherals::I2C0;
use embedded_graphics::{
    mono_font::{MonoTextStyle, MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use embedded_sdmmc::ShortFileName;
use heapless::String;
use ssd1306::{
    I2CDisplayInterface, Ssd1306, mode::BufferedGraphicsMode, prelude::*, size::DisplaySize128x64,
};

use crate::player::State;

type Oled = Ssd1306<
    I2CInterface<I2c<'static, I2C0, Blocking>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

/// Line height for the 6x10 font with a little padding.
const LINE_H: i32 = 11;

/// A buffered OLED we can render simple text screens onto.
pub struct Display {
    oled: Oled,
    style: MonoTextStyle<'static, BinaryColor>,
}

impl Display {
    /// Initialize the OLED on I2C0. Pins mirror the Python project:
    /// `scl` = SCL (GP13), `sda` = SDA (GP12). Returns `None` if the panel
    /// does not respond, so the firmware can still run headless.
    pub fn new(
        i2c0: Peri<'static, I2C0>,
        scl: Peri<'static, impl SclPin<I2C0>>,
        sda: Peri<'static, impl SdaPin<I2C0>>,
    ) -> Option<Self> {
        let mut config = i2c::Config::default();
        config.frequency = 400_000;

        let bus = I2c::new_blocking(i2c0, scl, sda, config);
        let interface = I2CDisplayInterface::new(bus);
        let mut oled = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();

        oled.init().ok()?;

        let style = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(BinaryColor::On)
            .build();

        Some(Self { oled, style })
    }

    /// Render up to four left-aligned lines and flush to the panel.
    fn draw_lines(&mut self, lines: &[&str]) {
        let _ = self.oled.clear(BinaryColor::Off);
        let mut y = 0;
        for line in lines {
            let _ = Text::with_baseline(line, Point::new(0, y), self.style, Baseline::Top)
                .draw(&mut self.oled);
            y += LINE_H;
        }
        let _ = self.oled.flush();
    }

    /// Boot splash.
    pub fn splash(&mut self) {
        self.draw_lines(&["MP3 Player", "Quadrature SDR", "", "booting..."]);
    }

    /// A two-line status/notification screen (title + body).
    pub fn message(&mut self, title: &str, body: &str) {
        self.draw_lines(&["MP3 Player", "", title, body]);
    }

    /// Live "now playing" screen: header, track index, filename, and state.
    pub fn now_playing(&mut self, index: usize, total: usize, name: &ShortFileName, state: State) {
        let mut header: String<20> = String::new();
        let _ = write!(header, "Track {}/{}", index + 1, total);

        let mut track: String<20> = String::new();
        let _ = write!(track, "{}", name);

        let status = match state {
            State::Playing => "> PLAYING",
            State::Paused => "|| PAUSED",
            State::Stopped => "[] STOPPED",
        };

        self.draw_lines(&["MP3 Player", header.as_str(), track.as_str(), status]);
    }
}
