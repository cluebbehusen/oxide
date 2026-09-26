//! The record shelf: two sections over one menu — SAVES (resumable
//! sessions, Enter loads) and REPLAYS (finished matches, Enter
//! watches) — the second screen-object extraction. Windowless update;
//! the main loop opens sessions and draws.

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

/// What one menu row stands for. Rows are values, not arithmetic: the
/// section headers shift every index below them, and a hand-shifted
/// offset is exactly the class of bug the pause screen's row enum
/// retired.
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
            _ => None,
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
                    // The honest version badge already told this story;
                    // the refusal just repeats it out loud.
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
mod tests {
    use super::*;
    use crate::saves::RecordKind;
    use macroquad::prelude::vec2;

    fn entry(
        name: &str,
        compatible: bool,
        kind: RecordKind,
        path: std::path::PathBuf,
    ) -> ReplayEntry {
        ReplayEntry {
            path,
            label: name.to_string(),
            blurb: format!("{name} blurb"),
            compatible,
            kind,
        }
    }

    fn drive(shelf: &mut Shelf, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        shelf.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            &mut mouse,
            &mut sounds,
        )
    }

    /// Moves the cursor to the labeled row and activates it — by label,
    /// so the tests survive the section headers shifting every index.
    fn activate(shelf: &mut Shelf, label: &str) -> Out {
        let target = shelf
            .menu
            .items
            .iter()
            .position(|i| i == label)
            .unwrap_or_else(|| panic!("no row labeled {label} in {:?}", shelf.menu.items));
        while shelf.menu.selected < target {
            drive(shelf, Key::Down);
        }
        while shelf.menu.selected > target {
            drive(shelf, Key::Up);
        }
        drive(shelf, Key::Enter)
    }

    #[test]
    fn the_back_button_leaves_even_after_every_record_is_deleted() {
        // The 0.9 regression, pinned structurally: however the shelf is
        // built — first open or post-delete rebuild — it has an exit.
        let mut shelf = Shelf::from_entries(vec![entry(
            "done",
            true,
            RecordKind::Match,
            "/nowhere/m.json".into(),
        )]);
        shelf.set_catalog(Vec::new());
        assert!(shelf.menu.items.is_empty());
        assert_eq!(drive(&mut shelf, Key::Enter), Out::Stay);
        for touch in [false, true] {
            let mut mouse = vec2(0.0, 0.0);
            let out = shelf.update(
                &crate::button::press_back(touch),
                &mut mouse,
                &mut Vec::new(),
            );
            assert_eq!(out, Out::Home);
        }
    }

    #[test]
    fn records_shelve_into_their_sections_and_the_cursor_skips_the_headers() {
        let mut shelf = Shelf::from_entries(vec![
            entry("live", true, RecordKind::Autosave, "/nowhere/a.json".into()),
            entry("named", true, RecordKind::Save, "/nowhere/s.json".into()),
            entry("done", true, RecordKind::Match, "/nowhere/m.json".into()),
        ]);
        assert_eq!(
            shelf.menu.items,
            vec!["SAVES", "live", "named", "REPLAYS", "done"],
            "saves first, replays after"
        );
        assert!(shelf.menu.is_header(0) && shelf.menu.is_header(3));
        assert_eq!(shelf.menu.selected, 1, "the cursor opens on a real row");
        // Walking down never rests on the REPLAYS header.
        drive(&mut shelf, Key::Down);
        drive(&mut shelf, Key::Down);
        assert_eq!(shelf.menu.items[shelf.menu.selected], "done");
    }

    #[test]
    fn enter_loads_a_save_and_watches_a_match() {
        let mut shelf = Shelf::from_entries(vec![
            entry("live", true, RecordKind::Autosave, "/nowhere/a.json".into()),
            entry("done", true, RecordKind::Match, "/nowhere/m.json".into()),
        ]);
        assert_eq!(
            activate(&mut shelf, "live"),
            Out::Load("/nowhere/a.json".into()),
            "a resumable record's verb is Load, never a mid-match scout"
        );
        assert_eq!(
            activate(&mut shelf, "done"),
            Out::Watch("/nowhere/m.json".into())
        );
    }

    #[test]
    fn deletion_requires_two_presses_and_is_dispatched_to_the_owner() {
        let dir = std::env::temp_dir().join(format!("oxide-shelf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doomed.json");
        std::fs::write(&path, "{}").unwrap();
        let mut shelf =
            Shelf::from_entries(vec![entry("doomed", true, RecordKind::Match, path.clone())]);
        assert_eq!(drive(&mut shelf, Key::X), Out::Stay, "first X only arms");
        assert!(path.exists(), "arming deletes nothing");
        assert_eq!(drive(&mut shelf, Key::X), Out::Delete(path.clone()));
        assert!(path.exists(), "the worker owns disk operations");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_incompatible_record_refuses_its_verb_in_both_sections() {
        let old_save = entry(
            "old-save",
            false,
            RecordKind::Save,
            "/nowhere/s.json".into(),
        );
        let old_match = entry(
            "old-match",
            false,
            RecordKind::Match,
            "/nowhere/m.json".into(),
        );
        let new = entry("new", true, RecordKind::Match, "/nowhere/new.json".into());
        let mut shelf = Shelf::from_entries(vec![old_save, old_match, new]);
        assert_eq!(
            activate(&mut shelf, "old-save"),
            Out::Stay,
            "an incompatible save refuses to load"
        );
        assert_eq!(
            activate(&mut shelf, "old-match"),
            Out::Stay,
            "an incompatible match refuses to watch"
        );
        assert_eq!(
            activate(&mut shelf, "new"),
            Out::Watch("/nowhere/new.json".into())
        );
    }
}
