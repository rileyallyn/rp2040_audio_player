//! I2S audio output for a PCM5102A DAC, driven by embassy-rp's built-in
//! [`PioI2sOut`] PIO program + DMA.
//!
//! Matches the working MicroPython `dac.py` configuration:
//! - 16-bit stereo Philips I2S at 44.1 kHz (32 BCK per frame)
//! - One 32-bit DMA word per stereo frame: left in bits 31:16, right in 15:0
//! - MCLK on GP17 via PWM (required by solder jumper JP14 on this board)
//!
//! `test_playback.py` uses 32-bit slots; the main player uses `dac.py` 16-bit.

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::dma::{self, Transfer};
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_rp::interrupt;
use embassy_rp::peripherals::PWM_SLICE0;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{Pio, PioPin};
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::pwm::{ChannelBPin, Config as PwmConfig, Pwm};
use embassy_rp::{dma::ChannelInstance, Peri};
use fixed::types::extra::U4;
use fixed::FixedU16;
use log::info;

/// Audio sample rate, matching `dac.py`.
pub const SAMPLE_RATE: u32 = 44_100;
/// Bit clocks per channel (16 -> 32 BCK per stereo frame).
pub const BITS_PER_CHANNEL: u32 = 16;
/// PCM5102A MCLK multiplier, matching `test_playback.py`.
pub const MCLK_RATIO: u32 = 256;
/// Drive GP17 with PWM MCLK. Required: solder jumper JP14 ("SCK_SRC_JUMPER",
/// bridged 1-2 by default) hardwires the PCM5102A SCK input to GP17.
pub const ENABLE_MCLK: bool = true;

/// Stereo frames per DMA buffer for [`Dac::stream`].
pub const STREAM_BUF_FRAMES: usize = 1024;

/// Pack a stereo frame into one 32-bit I2S word (left in 31:16, right in 15:0).
///
/// This is the layout MicroPython `machine.I2S` uses for 16-bit stereo TX and
/// matches `struct.pack_into("<hh", buf, ...)`.
#[inline]
pub fn pack_frame(left: i16, right: i16) -> u32 {
    ((left as u16 as u32) << 16) | (right as u16 as u32)
}

/// Convenience: same sample to both channels (dual-mono).
#[inline]
pub fn pack_mono(sample: i16) -> u32 {
    pack_frame(sample, sample)
}

/// PCM5102A DAC over embassy-rp's PIO I2S transmitter with a hardware mute (XSMT) line.
pub struct Dac {
    i2s: PioI2sOut<'static, PIO0, 0>,
    /// Keeps the loaded PIO program alive for the life of the DAC.
    _i2s_prog: PioI2sOutProgram<'static, PIO0>,
    mute_ctrl: Output<'static>,
    /// Keeps the MCLK PWM slice running for the life of the DAC (if enabled).
    _mclk: Option<Pwm<'static>>,
}

/// PWM settings for GP17 MCLK: `sample_rate * 256` Hz at ~50% duty.
///
/// Replicates MicroPython's `machine.PWM` (the proven-working MCLK on this
/// board): divider 1.0 and the wrap count nearest the target.
fn mclk_pwm_config(sample_rate: u32) -> PwmConfig {
    let target_hz = sample_rate * MCLK_RATIO;
    let wrap = (clk_sys_freq() + target_hz / 2) / target_hz;
    let wrap = wrap.clamp(2, 65_536);

    let mut cfg = PwmConfig::default();
    cfg.top = (wrap - 1) as u16;
    cfg.divider = FixedU16::<U4>::from_num(1);
    cfg.compare_a = 0;
    cfg.compare_b = (wrap / 2) as u16;
    cfg.enable = true;
    cfg
}

impl Dac {
    /// Configure PIO0/SM0 for I2S output, optional MCLK PWM, and the mute pin.
    ///
    /// Pins mirror the Python project:
    /// - `data` = DIN (GP14), `bit_clock` = BCK (GP15), `lr_clock` = LRCK/WS (GP16)
    /// - `mute` = XSMT (GP6)
    /// - `mclk` = master clock (GP17), via `PWM_SLICE0` channel B
    pub fn new<D: ChannelInstance>(
        pio: Pio<'static, PIO0>,
        dma: Peri<'static, D>,
        data_pin: Peri<'static, impl PioPin>,
        bit_clock_pin: Peri<'static, impl PioPin>,
        lr_clock_pin: Peri<'static, impl PioPin>,
        mute_pin: Peri<'static, impl Pin>,
        mclk_slice: Peri<'static, PWM_SLICE0>,
        mclk_pin: Peri<'static, impl ChannelBPin<PWM_SLICE0>>,
        irq: impl interrupt::typelevel::Binding<D::Interrupt, dma::InterruptHandler<D>> + 'static,
    ) -> Self {
        let Pio {
            mut common, sm0, ..
        } = pio;

        let i2s_prog = PioI2sOutProgram::new(&mut common);

        let mut i2s = PioI2sOut::new(
            &mut common,
            sm0,
            dma,
            irq,
            data_pin,
            bit_clock_pin,
            lr_clock_pin,
            SAMPLE_RATE,
            BITS_PER_CHANNEL,
            &i2s_prog,
        );

        i2s.start();

        let mute_ctrl = Output::new(mute_pin, Level::Low);

        let mclk = if ENABLE_MCLK {
            let target_hz = SAMPLE_RATE * MCLK_RATIO;
            let pwm = Pwm::new_output_b(mclk_slice, mclk_pin, mclk_pwm_config(SAMPLE_RATE));
            info!("MCLK on GP17: target {} Hz", target_hz);
            Some(pwm)
        } else {
            info!("MCLK disabled");
            None
        };

        info!(
            "I2S: 16-bit, {} BCK/frame, BCK ~{} Hz, LRCK {} Hz",
            BITS_PER_CHANNEL * 2,
            SAMPLE_RATE * BITS_PER_CHANNEL * 2,
            SAMPLE_RATE
        );

        Self {
            i2s,
            _i2s_prog: i2s_prog,
            mute_ctrl,
            _mclk: mclk,
        }
    }

    /// Pull XSMT low to soft-mute the DAC output.
    pub fn mute(&mut self) {
        self.mute_ctrl.set_low();
    }

    /// Pull XSMT high to unmute the DAC output.
    pub fn unmute(&mut self) {
        self.mute_ctrl.set_high();
    }

    /// Queue packed stereo frames for DMA into the PIO TX FIFO.
    pub fn write<'a>(&'a mut self, buffer: &'a [u32]) -> Transfer<'a> {
        self.i2s.write(buffer)
    }

    /// Block until `buffer` has been clocked out.
    pub async fn drain(&mut self, buffer: &[u32]) {
        self.write(buffer).await;
    }

    /// Run BCK/LRCK with silence, then unmute.
    pub async fn start_output(&mut self) {
        const SILENCE: [u32; 512] = [0; 512];
        for _ in 0..3 {
            self.drain(&SILENCE).await;
        }
        self.unmute();
        info!("DAC unmuted, I2S running");
    }

    /// Gapless ping-pong stream until `fill_back` returns false (end of input).
    pub async fn stream(
        &mut self,
        buf_a: &mut [u32],
        buf_b: &mut [u32],
        mut fill_back: impl FnMut(&mut [u32]) -> bool,
    ) {
        let mut front: &mut [u32] = buf_a;
        let mut back: &mut [u32] = buf_b;

        if !fill_back(front) {
            return;
        }

        loop {
            let transfer = self.write(front);
            let has_more = fill_back(back);
            transfer.await;
            core::mem::swap(&mut front, &mut back);
            if !has_more {
                self.drain(front).await;
                break;
            }
        }
    }
}
