//! The record shelf: two sections over one menu, SAVES (resumable
//! sessions, Enter loads) and REPLAYS (finished matches, Enter watches).
//! Windowless update; the main loop opens sessions and draws.

use crate::game::SoundKind;
use crate::menu::{Label, Line, Menu};
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

/// The shelf screen: discovered records, their sectioned menu, and the
/// two-press delete arming state.
pub struct Shelf {
    /// Everything loadable, watchable, or deletable, newest first
    /// within its section.
    pub entries: Vec<ReplayEntry>,
    pub(crate) catalog_ready: bool,
    /// The rows: section headers and one row per entry, standing for its
    /// index into `entries`. The exit is the BACK button outside the
    /// rows, so no delete-refresh can drop it and strand a mouse-only
    /// player in an exitless menu.
    pub menu: Menu<usize>,
    /// Entry armed for deletion; X on the same entry confirms.
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
        let selected = self.menu.value().map(|i| self.entries[*i].path.clone());
        let mut fresh = Self::from_entries(entries);
        if let Some(path) = selected {
            let paths: Vec<_> = fresh.entries.iter().map(|e| e.path.clone()).collect();
            fresh.menu.select_where(|i| paths[*i] == path);
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
        let mut lines = Vec::new();
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
            lines.push(Line::Header(title.to_string()));
            for i in section {
                lines.push(Line::Row(Label::Text(entries[i].label.clone()), i));
            }
        }
        Self {
            entries,
            catalog_ready: true,
            menu: Menu::new("SAVES & REPLAYS", lines),
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
            return match self.entries.get(row.value) {
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
            && let Some(&i) = self.menu.value()
        {
            if self.arming == Some(i) {
                self.arming = None;
                return Out::Delete(self.entries[i].path.clone());
            }
            self.arming = Some(i);
        }
        Out::Stay
    }

    /// How to act on the focused row: coaching for the footer. None while
    /// a delete is armed, since the subtitle already says what to press.
    pub fn coaching(&self) -> Option<String> {
        let i = *self.menu.value()?;
        if self.arming == Some(i) {
            return None;
        }
        self.entries
            .get(i)
            .map(|entry| entry.hint(crate::platform::hands()))
            .filter(|hint| !hint.is_empty())
    }

    /// The focused row's detail line.
    pub fn subtitle(&self) -> String {
        if self.entries.is_empty() {
            "nothing recorded yet: finish a match or quit one mid-way".to_string()
        } else if self.arming.is_some() && self.arming == self.menu.value().copied() {
            "press {delete} again to delete this record".to_string()
        } else {
            self.menu
                .value()
                .and_then(|i| self.entries.get(*i))
                .map(|e| e.blurb.clone())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests;
