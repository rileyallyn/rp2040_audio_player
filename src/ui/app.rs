//! Main UI loop, ported from `menu.py` `SDRApp`.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Timer;

use crate::display::Display;
use crate::input::{encoder_counter, take_button_events};
use crate::player::{
    playback_state, PlaybackController, PlayRequestSignal, PlaybackDoneSignal, PlayerControl,
    State,
};
use crate::sd::TrackList;
use crate::ui::nav::{Page, NavStack};
use crate::ui::pages::{draw_page, handle_page_input, stop_if_playing, InputResult};

type PlayerMutex = Mutex<CriticalSectionRawMutex, PlaybackController>;

const TICK_MS: u64 = 50;

pub struct UiApp {
    nav: NavStack,
    tracks: TrackList,
    last_encoder: i32,
    waiting_for_stop: bool,
}

impl UiApp {
    pub fn new(tracks: TrackList) -> Self {
        let nav = NavStack::new(Page::track_list(&tracks));
        Self {
            nav,
            tracks,
            last_encoder: encoder_counter(),
            waiting_for_stop: false,
        }
    }

    pub fn refresh_tracks(&mut self, player: &PlaybackController) {
        player.list_tracks(&mut self.tracks);
        self.nav.current().refresh_track_list(&self.tracks);
    }

    pub async fn run(
        &mut self,
        mut display: Option<&mut Display>,
        player: &PlayerMutex,
        control: &PlayerControl,
        play_request: &PlayRequestSignal,
        playback_done: &PlaybackDoneSignal,
    ) -> ! {
        if let Some(d) = display.as_deref_mut() {
            draw_page(self.nav.current(), d, &self.tracks);
        }

        loop {
            let current_counter = encoder_counter();
            let diff = current_counter - self.last_encoder;
            self.last_encoder = current_counter;

            let (clicked, long_press) = take_button_events();

            let mut redraw = diff != 0 || clicked;

            if self.waiting_for_stop {
                if playback_done.try_take().is_some() || playback_state() == State::Stopped {
                    self.waiting_for_stop = false;
                    let _ = self.nav.pop();
                    self.nav.current().refresh_track_list(&self.tracks);
                    redraw = true;
                }
            } else if self.nav.is_playback() && playback_done.try_take().is_some() {
                let _ = self.nav.pop();
                self.nav.current().refresh_track_list(&self.tracks);
                redraw = true;
            }

            if long_press && self.nav.can_pop() {
                if self.nav.is_playback() {
                    stop_if_playing(control);
                    self.waiting_for_stop = true;
                } else {
                    let _ = self.nav.pop();
                }
                redraw = true;
            } else if !self.waiting_for_stop && (diff != 0 || clicked) {
                match handle_page_input(
                    self.nav.current(),
                    &self.tracks,
                    diff,
                    clicked,
                    control,
                    playback_done,
                ) {
                    InputResult::Redraw => {}
                    InputResult::StartPlayback(req) => {
                        play_request.signal(req.clone());
                        let page = Page::playback(req.name, req.index, req.total);
                        self.nav.push(page);
                        redraw = true;
                    }
                    InputResult::StopPlayback => {
                        self.waiting_for_stop = true;
                    }
                    InputResult::PopAfterStop => {
                        let _ = self.nav.pop();
                        self.nav.current().refresh_track_list(&self.tracks);
                    }
                }
            }

            if redraw {
                if let Some(d) = display.as_deref_mut() {
                    draw_page(self.nav.current(), d, &self.tracks);
                }
            }

            // Periodically refresh the track list while at root with no tracks.
            if matches!(self.nav.current(), Page::TrackList { .. }) && self.tracks.is_empty() {
                {
                    let p = player.lock().await;
                    self.refresh_tracks(&*p);
                }
                if let Some(d) = display.as_deref_mut() {
                    draw_page(self.nav.current(), d, &self.tracks);
                }
            }

            Timer::after_millis(TICK_MS).await;
        }
    }
}
