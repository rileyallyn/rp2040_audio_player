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
    primitives::{Line, PrimitiveStyle},
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

    /// Scrollable menu with title, rule, and up to four option rows.
    pub fn draw_menu(
        &mut self,
        title: &str,
        options: &[String<16>],
        selected: usize,
        scroll_offset: usize,
    ) {
        const MAX_VISIBLE: usize = 4;

        let _ = self.oled.clear(BinaryColor::Off);
        let _ = Text::with_baseline(title, Point::new(0, 0), self.style, Baseline::Top)
            .draw(&mut self.oled);
        let _ = Line::new(
            Point::new(0, 10),
            Point::new(127, 10),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(&mut self.oled);

        let mut y = 15;
        if options.is_empty() {
            let _ = Text::with_baseline("  (Empty)", Point::new(0, y), self.style, Baseline::Top)
                .draw(&mut self.oled);
        } else {
            let end = (scroll_offset + MAX_VISIBLE).min(options.len());
            for (i, opt) in options.iter().enumerate().skip(scroll_offset).take(end - scroll_offset)
            {
                let prefix = if i == selected { '>' } else { ' ' };
                let mut line: String<20> = String::new();
                let _ = write!(line, "{} {}", prefix, opt.as_str());
                let text = if line.len() > 16 { &line[..16] } else { line.as_str() };
                let _ = Text::with_baseline(text, Point::new(0, y), self.style, Baseline::Top)
                    .draw(&mut self.oled);
                y += 12;
            }
        }
        let _ = self.oled.flush();
    }

    /// Playback screen: title, filename, and control options.
    pub fn draw_playback(
        &mut self,
        title: &str,
        filename: &str,
        options: &heapless::Vec<&str, 2>,
        selected: usize,
    ) {
        let _ = self.oled.clear(BinaryColor::Off);
        let _ = Text::with_baseline(title, Point::new(0, 0), self.style, Baseline::Top)
            .draw(&mut self.oled);
        let _ = Line::new(
            Point::new(0, 10),
            Point::new(127, 10),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(&mut self.oled);

        let name = if filename.len() > 16 {
            &filename[..16]
        } else {
            filename
        };
        let _ = Text::with_baseline(name, Point::new(0, 15), self.style, Baseline::Top)
            .draw(&mut self.oled);

        let mut y = 30;
        for (i, opt) in options.iter().enumerate() {
            let prefix = if i == selected { '>' } else { ' ' };
            let mut line: String<20> = String::new();
            let _ = write!(line, "{} {}", prefix, opt);
            let _ = Text::with_baseline(line.as_str(), Point::new(0, y), self.style, Baseline::Top)
                .draw(&mut self.oled);
            y += 12;
        }
        let _ = self.oled.flush();
    }

    /// Draw a single overlay line without clearing the framebuffer.
    pub fn draw_overlay_line(&mut self, text: &str, y: i32) {
        let _ = Text::with_baseline(text, Point::new(0, y), self.style, Baseline::Top)
            .draw(&mut self.oled);
        let _ = self.oled.flush();
    }
}
