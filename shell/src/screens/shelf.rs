//! The record shelf: two sections over one menu, SAVES (resumable
//! sessions, Enter loads) and REPLAYS (finished matches, Enter watches).
//! Windowless update; the main loop opens sessions and draws.

use crate::game::SoundKind;
use crate::menu::Menu;
use crate::saves::ReplayEntry;
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

/// What a shelf frame decided.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Still browsing.
    Stay,
    /// Back to the front door.
    Home,
    /// Resume this record as a live session (the caller loads it and
    /// answers for a file that no longer loads).
    Load(std::path::PathBuf),
    /// Watch this record (the caller opens the playback session and
    /// answers for a file that no longer loads).
    Watch(std::path::PathBuf),
    /// A record was deleted; the caller rebuilds the shelf and the
    /// Home menu (the deleted file may have been Continue's save).
    Delete(std::path::PathBuf),
}

/// What one menu row stands for. Rows are values, not index arithmetic,
/// because the section headers shift every index below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// A section label; the cursor skips it, clicks ignore it.
    Header,
    /// A record, by index into `entries`.
    Entry(usize),
}

/// The shelf screen: discovered records, their sectioned menu, and the
/// two-press delete arming state.
pub struct Shelf {
    /// Everything loadable, watchable, or deletable, newest first
    /// within its section.
    pub entries: Vec<ReplayEntry>,
    pub(crate) catalog_ready: bool,
    /// The rows: section headers and one row per entry. The exit is the
    /// BACK button outside the rows, so no delete-refresh can drop it
    /// and strand a mouse-only player in an exitless menu.
    pub menu: Menu,
    /// What each menu row stands for, parallel to `menu.items`.
    rows: Vec<RowKind>,
    /// Menu row armed for deletion; X on the same row confirms.
    pub arming: Option<usize>,
    back: crate::button::BackButton,
}

impl Shelf {
    /// Scans the save and replay directories like the front door does.
    pub fn open() -> Self {
        let mut shelf = Self::from_entries(Vec::new());
        shelf.catalog_ready = false;
        shelf
    }

    pub(crate) fn set_catalog(&mut self, entries: Vec<ReplayEntry>) {
        let selected = self.rows.get(self.menu.selected).and_then(|row| match row {
            RowKind::Entry(i) => Some(self.entries[*i].path.clone()),
            RowKind::Header => None,
        });
        let mut fresh = Self::from_entries(entries);
        if let Some(path) = selected
            && let Some(row) = fresh
                .rows
                .iter()
                .position(|row| matches!(row, RowKind::Entry(i) if fresh.entries[*i].path == path))
        {
            fresh.menu.select(row);
        }
        fresh.catalog_ready = true;
        // A refresh landing mid-press must not drop the BACK gesture.
        fresh.back = std::mem::take(&mut self.back);
        *self = fresh;
    }

    /// Builds the shelf over the given records (tests inject their own).
    /// Sections appear only when they have rows; an empty shelf is just
    /// the empty-state subtitle.
    pub fn from_entries(entries: Vec<ReplayEntry>) -> Self {
        let mut items: Vec<String> = Vec::new();
        let mut rows: Vec<RowKind> = Vec::new();
        let mut headers: Vec<usize> = Vec::new();
        for (title, resumable) in [("SAVES", true), ("REPLAYS", false)] {
            let section: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.kind.resumable() == resumable)
                .map(|(i, _)| i)
                .collect();
            if section.is_empty() {
                continue;
            }
            headers.push(items.len());
            items.push(title.to_string());
            rows.push(RowKind::Header);
            for i in section {
                items.push(entries[i].label.clone());
                rows.push(RowKind::Entry(i));
            }
        }
        Self {
            entries,
            catalog_ready: true,
            menu: Menu::with_headers("SAVES & REPLAYS", items, headers),
            rows,
            arming: None,
            back: crate::button::BackButton::default(),
        }
    }

    /// Applies a frame's events. Deletion happens here (two X presses
    /// on the same row); everything session-shaped is returned to the
    /// caller as an [`Out`].
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        let escaped = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
        let x_pressed = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::X }));
        let (back, events) = self.back.route(events);
        if back {
            sounds.push((SoundKind::Click, None));
            return Out::Home;
        }
        let picked = self.menu.handle(&events, mouse);
        if escaped {
            return Out::Home;
        }
        if let Some(row) = picked {
            sounds.push((SoundKind::Click, None));
            let entry = match self.rows.get(row) {
                Some(RowKind::Entry(i)) => self.entries.get(*i),
                _ => return Out::Stay,
            };
            return match entry {
                Some(entry) if entry.compatible && entry.kind.resumable() => {
                    Out::Load(entry.path.clone())
                }
                Some(entry) if entry.compatible => Out::Watch(entry.path.clone()),
                Some(_) => {
                    // The row's version badge already says why.
                    sounds.push((SoundKind::Denied, None));
                    Out::Stay
                }
                None => Out::Stay,
            };
        }
        if x_pressed
            && self.catalog_ready
            && let Some(RowKind::Entry(i)) = self.rows.get(self.menu.selected).copied()
        {
            let row = self.menu.selected;
            if self.arming == Some(row) {
                self.arming = None;
                return Out::Delete(self.entries[i].path.clone());
            }
            self.arming = Some(row);
        }
        Out::Stay
    }

    /// How to act on the focused row: coaching for the footer. None while
    /// a delete is armed, since the subtitle already says what to press.
    pub fn coaching(&self) -> Option<String> {
        if self.arming == Some(self.menu.selected) {
            return None;
        }
        match self.rows.get(self.menu.selected) {
            Some(RowKind::Entry(i)) => self
                .entries
                .get(*i)
                .map(|entry| entry.hint.clone())
                .filter(|hint| !hint.is_empty()),
            _ => None,
        }
    }

    /// The focused row's detail line.
    pub fn subtitle(&self) -> String {
        if self.entries.is_empty() {
            "nothing recorded yet: finish a match or quit one mid-way".to_string()
        } else if self.arming == Some(self.menu.selected) {
            "press {delete} again to delete this record".to_string()
        } else {
            match self.rows.get(self.menu.selected) {
                Some(RowKind::Entry(i)) => self
                    .entries
                    .get(*i)
                    .map(|e| e.blurb.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests;
