//! Page input handlers and drawing, ported from `ui/music_pages.py`.

use core::fmt::Write;

use heapless::String;

use crate::display::Display;
use crate::player::{
    playback_state, Command, PlayRequest, PlaybackDoneSignal, PlayerControl,
    State,
};
use crate::sd::TrackList;
use crate::ui::menu::Menu;
use crate::ui::nav::Page;

/// Result of handling one input event on the current page.
pub enum InputResult {
    Redraw,
    StartPlayback(PlayRequest),
    StopPlayback,
    PopAfterStop,
}

pub fn draw_page(page: &mut Page, display: &mut Display, tracks: &TrackList) {
    match page {
        Page::TrackList { menu } => {
            display.draw_menu(menu.title, menu.options.as_slice(), menu.selected_idx, menu.scroll_offset);
            if tracks.is_empty() {
                display.draw_overlay_line("  (No files)", 27);
            }
        }
        Page::Playback { name, menu, .. } => {
            sync_playback_label(menu);
            let mut filename: String<20> = String::new();
            let _ = write!(filename, "{}", name);
            let options = playback_menu_labels(menu);
            display.draw_playback(menu.title, filename.as_str(), &options, menu.selected_idx);
        }
    }
}

fn playback_menu_labels(menu: &Menu<2>) -> heapless::Vec<&str, 2> {
    let mut labels = heapless::Vec::new();
    for (i, opt) in menu.options.iter().enumerate() {
        let label = if i == 0 {
            match playback_state() {
                State::Playing => "Pause",
                _ => "Play",
            }
        } else {
            opt.as_str()
        };
        let _ = labels.push(label);
    }
    labels
}

pub fn handle_page_input(
    page: &mut Page,
    tracks: &TrackList,
    diff: i32,
    clicked: bool,
    control: &PlayerControl,
    playback_done: &PlaybackDoneSignal,
) -> InputResult {
    if let Page::Playback { menu, .. } = page {
        sync_playback_label(menu);
        if playback_done.try_take().is_some() {
            return InputResult::PopAfterStop;
        }
    }

    if diff > 0 {
        match page {
            Page::TrackList { menu } => menu.next(),
            Page::Playback { menu, .. } => menu.next(),
        }
    } else if diff < 0 {
        match page {
            Page::TrackList { menu } => menu.prev(),
            Page::Playback { menu, .. } => menu.prev(),
        }
    }

    if !clicked {
        return InputResult::Redraw;
    }

    match page {
        Page::TrackList { menu } => {
            let idx = menu.selected_idx;
            if let Some(name) = tracks.get(idx).cloned() {
                let total = tracks.len();
                InputResult::StartPlayback(PlayRequest {
                    name,
                    index: idx,
                    total,
                })
            } else {
                InputResult::Redraw
            }
        }
        Page::Playback { menu, .. } => {
            match menu.selected_idx {
                0 => {
                    let _ = control.signal(Command::Toggle);
                    InputResult::Redraw
                }
                1 => {
                    let _ = control.signal(Command::Stop);
                    InputResult::StopPlayback
                }
                _ => InputResult::Redraw,
            }
        }
    }
}

fn sync_playback_label(menu: &mut Menu<2>) {
    if menu.options.len() >= 1 {
        let label = match playback_state() {
            State::Playing => "Pause",
            _ => "Play",
        };
        let _ = menu.options[0].clear();
        let _ = menu.options[0].push_str(label);
    }
}

pub fn stop_if_playing(control: &PlayerControl) {
    if playback_state() != State::Stopped {
        control.signal(Command::Stop);
    }
}
