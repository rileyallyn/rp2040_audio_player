//! Scrollable menu widget, ported from `menu.py` `Menu` class.

use heapless::String;

/// Number of option rows visible below the title line.
pub const MAX_VISIBLE: usize = 4;

/// Maximum characters per menu row (128 px / 8 px per char).
pub const MAX_ROW_CHARS: usize = 16;

/// A scrollable selectable list of text options.
pub struct Menu<const N: usize> {
    pub title: &'static str,
    pub options: heapless::Vec<String<{ MAX_ROW_CHARS }>, N>,
    pub selected_idx: usize,
    pub scroll_offset: usize,
}

impl<const N: usize> Menu<N> {
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            options: heapless::Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
        }
    }

    pub fn set_options<I>(&mut self, labels: I)
    where
        I: IntoIterator<Item = String<{ MAX_ROW_CHARS }>>,
    {
        self.options.clear();
        for label in labels {
            let _ = self.options.push(label);
        }
        self.selected_idx = 0;
        self.scroll_offset = 0;
    }

    pub fn next(&mut self) {
        if self.options.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.options.len();
        self.adjust_scroll();
    }

    pub fn prev(&mut self) {
        if self.options.is_empty() {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.options.len() - 1;
        } else {
            self.selected_idx -= 1;
        }
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + MAX_VISIBLE {
            self.scroll_offset = self.selected_idx - MAX_VISIBLE + 1;
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.options.get(self.selected_idx).map(|s| s.as_str())
    }
}
