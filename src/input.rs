//! Rotary encoder and center-button input, ported from
//! `Bringup/rotary_encoder.py` and wired like `menu.py` (GP20/21/22).

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_rp::Peri;
use embassy_rp::gpio::{Input, Pull};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};

/// Encoder A (GP20) and B (GP21), matching `menu.py`.
const DEBOUNCE_MS: u64 = 20;
const LONG_PRESS_MS: u64 = 600;

struct InputState {
    encoder_counter: i32,
    click_pending: bool,
    long_pending: bool,
}

impl InputState {
    const fn new() -> Self {
        Self {
            encoder_counter: 0,
            click_pending: false,
            long_pending: false,
        }
    }
}

static INPUT: Mutex<CriticalSectionRawMutex, RefCell<InputState>> =
    Mutex::new(RefCell::new(InputState::new()));

/// Current detent counter (poll and diff against a saved value in the UI loop).
pub fn encoder_counter() -> i32 {
    INPUT.lock(|state| state.borrow().encoder_counter)
}

/// Return `(clicked, long_press)` since the last call and clear pending flags.
pub fn take_button_events() -> (bool, bool) {
    INPUT.lock(|state| {
        let mut state = state.borrow_mut();
        let clicked = state.click_pending;
        let long_press = state.long_pending;
        state.click_pending = false;
        state.long_pending = false;
        (clicked, long_press)
    })
}

fn add_encoder_steps(steps: i32) {
    INPUT.lock(|state| state.borrow_mut().encoder_counter += steps);
}

fn set_click_pending() {
    INPUT.lock(|state| state.borrow_mut().click_pending = true);
}

fn set_long_pending() {
    INPUT.lock(|state| state.borrow_mut().long_pending = true);
}

/// Quadrature decode on pin A transitions; one detent per full cycle (`ppr = 1`).
async fn encoder_loop(mut pin_a: Input<'static>, pin_b: Input<'static>) -> ! {
    let mut last_a = pin_a.is_high();
    let mut step_count: i32 = 0;
    const PPR: i32 = 1;

    loop {
        pin_a.wait_for_any_edge().await;
        let current_a = pin_a.is_high();
        let current_b = pin_b.is_high();

        if current_a != last_a {
            if current_b != current_a {
                step_count += 1;
            } else {
                step_count -= 1;
            }

            if step_count.abs() >= PPR {
                let complete = step_count / PPR;
                add_encoder_steps(complete);
                step_count -= complete * PPR;
            }
        }

        last_a = current_a;
    }
}

/// Debounced short click and long-press detection on the center button.
async fn button_loop(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        Timer::after_millis(DEBOUNCE_MS).await;

        if button.is_high() {
            continue;
        }

        let down_at = Instant::now();
        let mut long_sent = false;

        loop {
            if button.is_high() {
                let held = down_at.elapsed();
                if !long_sent && held < Duration::from_millis(LONG_PRESS_MS) {
                    set_click_pending();
                }
                break;
            }

            if !long_sent && down_at.elapsed() >= Duration::from_millis(LONG_PRESS_MS) {
                set_long_pending();
                long_sent = true;
            }

            Timer::after_millis(10).await;
        }
    }
}

#[embassy_executor::task]
async fn encoder_task(
    pin_a: Peri<'static, embassy_rp::gpio::AnyPin>,
    pin_b: Peri<'static, embassy_rp::gpio::AnyPin>,
) -> ! {
    let pin_a = Input::new(pin_a, Pull::Up);
    let pin_b = Input::new(pin_b, Pull::Up);
    encoder_loop(pin_a, pin_b).await
}

#[embassy_executor::task]
async fn button_task(button: Peri<'static, embassy_rp::gpio::AnyPin>) -> ! {
    let button = Input::new(button, Pull::Up);
    button_loop(button).await
}

/// Spawn encoder and button tasks on GP20, GP21, GP22.
pub fn spawn(
    spawner: &Spawner,
    pin_a: Peri<'static, embassy_rp::gpio::AnyPin>,
    pin_b: Peri<'static, embassy_rp::gpio::AnyPin>,
    button: Peri<'static, embassy_rp::gpio::AnyPin>,
) {
    spawner.must_spawn(encoder_task(pin_a, pin_b));
    spawner.must_spawn(button_task(button));
}
