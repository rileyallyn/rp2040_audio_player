//! Page stack navigation, ported from `ui/page.py`.

use core::fmt::Write;

use embedded_sdmmc::ShortFileName;
use heapless::String;

use crate::sd::TrackList;
use crate::ui::menu::{Menu, MAX_ROW_CHARS};

const MAX_STACK: usize = 4;

/// Navigable screen variants for the music-only UI.
pub enum Page {
    TrackList {
        menu: Menu<{ crate::sd::MAX_TRACKS }>,
    },
    Playback {
        name: ShortFileName,
        index: usize,
        total: usize,
        menu: Menu<2>,
    },
}

impl Page {
    pub fn track_list(tracks: &TrackList) -> Self {
        let mut menu = Menu::new("Music Player");
        menu.set_options(track_labels(tracks));
        Self::TrackList { menu }
    }

    pub fn refresh_track_list(&mut self, tracks: &TrackList) {
        if let Self::TrackList { menu } = self {
            menu.set_options(track_labels(tracks));
        }
    }

    pub fn playback(name: ShortFileName, index: usize, total: usize) -> Self {
        let mut menu = Menu::new("Playing:");
        let mut pause = String::<MAX_ROW_CHARS>::new();
        let _ = pause.push_str("Pause");
        let mut exit = String::<MAX_ROW_CHARS>::new();
        let _ = exit.push_str("Exit");
        menu.set_options([pause, exit]);
        Self::Playback {
            name,
            index,
            total,
            menu,
        }
    }
}

pub fn track_labels(
    tracks: &TrackList,
) -> heapless::Vec<String<MAX_ROW_CHARS>, { crate::sd::MAX_TRACKS }> {
    let mut labels = heapless::Vec::new();
    for i in 0..tracks.len() {
        if let Some(name) = tracks.get(i) {
            let mut label = String::<MAX_ROW_CHARS>::new();
            truncate_name(name, &mut label);
            let _ = labels.push(label);
        }
    }
    labels
}

fn truncate_name(name: &ShortFileName, out: &mut String<MAX_ROW_CHARS>) {
    let _ = write!(out, "{}", name);
    if out.len() > MAX_ROW_CHARS {
        out.truncate(MAX_ROW_CHARS);
    }
}

/// Stack-based navigation for nested pages.
pub struct NavStack {
    stack: heapless::Vec<Page, MAX_STACK>,
}

impl NavStack {
    pub fn new(root: Page) -> Self {
        let mut stack = heapless::Vec::new();
        let _ = stack.push(root);
        Self { stack }
    }

    pub fn push(&mut self, page: Page) {
        let _ = self.stack.push(page);
    }

    pub fn pop(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        let _ = self.stack.pop();
        true
    }

    pub fn can_pop(&self) -> bool {
        self.stack.len() > 1
    }

    pub fn current(&mut self) -> &mut Page {
        self.stack.last_mut().unwrap()
    }

    pub fn is_playback(&self) -> bool {
        matches!(self.stack.last(), Some(Page::Playback { .. }))
    }
}
