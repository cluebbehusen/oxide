//! The replay shelf: autosaves and local records, watch or delete —
//! the second screen-object extraction. Windowless update; the main
//! loop opens playback sessions and draws.

use crate::game::SoundKind;
use crate::menu::Menu;
use crate::saves::{self, ReplayEntry};
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

/// What a shelf frame decided.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// Still browsing.
    Stay,
    /// Back to the front door.
    Home,
    /// Watch this record (the caller opens the playback session and
    /// answers for a file that no longer loads).
    Watch(std::path::PathBuf),
    /// A record was deleted; the caller rebuilds the shelf and the
    /// Home menu (the deleted file may have been Continue's save).
    Deleted,
}

/// The shelf screen: discovered records, their menu, and the
/// two-press delete arming state.
pub struct Shelf {
    /// Everything watchable or deletable, newest first.
    pub entries: Vec<ReplayEntry>,
    /// The rows: one per entry, plus Back — always plus Back, which is
    /// the 0.9 regression this construction pins (a delete-refresh once
    /// dropped it and stranded mouse-only players in an exitless menu).
    pub menu: Menu,
    /// Row armed for deletion; X on the same row confirms.
    pub arming: Option<usize>,
}

impl Shelf {
    /// Scans the autosave dir and `replays/` like the front door does.
    pub fn open() -> Self {
        Self::from_entries(saves::discover())
    }

    /// Builds the shelf over the given records (tests inject their own).
    pub fn from_entries(entries: Vec<ReplayEntry>) -> Self {
        let mut rows: Vec<String> = entries.iter().map(|e| e.label.clone()).collect();
        rows.push("Back".to_string());
        Self {
            entries,
            menu: Menu::new("REPLAYS", rows),
            arming: None,
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
        let picked = self.menu.handle(events, mouse);
        if escaped {
            return Out::Home;
        }
        if let Some(row) = picked {
            sounds.push((SoundKind::Click, None));
            if row >= self.entries.len() {
                return Out::Home;
            }
            return match self.entries.get(row) {
                Some(entry) if entry.compatible => Out::Watch(entry.path.clone()),
                Some(_) => {
                    sounds.push((SoundKind::Denied, None));
                    Out::Stay
                }
                None => Out::Stay,
            };
        }
        if x_pressed && self.menu.selected < self.entries.len() {
            let row = self.menu.selected;
            if self.arming == Some(row) {
                if let Some(entry) = self.entries.get(row) {
                    std::fs::remove_file(&entry.path).ok();
                }
                self.arming = None;
                return Out::Deleted;
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
            "press X again to delete this record".to_string()
        } else {
            self.entries
                .get(self.menu.selected)
                .map(|e| e.blurb.clone())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn entry(name: &str, compatible: bool, path: std::path::PathBuf) -> ReplayEntry {
        ReplayEntry {
            path,
            label: name.to_string(),
            blurb: format!("{name} blurb"),
            compatible,
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

    #[test]
    fn the_back_row_exists_even_after_every_record_is_deleted() {
        // The 0.9 regression, pinned structurally: however the shelf is
        // built — first open or post-delete rebuild — Back is a row.
        let empty = Shelf::from_entries(Vec::new());
        assert_eq!(empty.menu.items.last().map(String::as_str), Some("Back"));
        let mut shelf = empty;
        assert_eq!(drive(&mut shelf, Key::Enter), Out::Home, "Back activates");
    }

    #[test]
    fn deleting_takes_two_x_presses_on_the_same_row_and_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("oxide-shelf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doomed.json");
        std::fs::write(&path, "{}").unwrap();
        let mut shelf = Shelf::from_entries(vec![entry("doomed", true, path.clone())]);
        assert_eq!(drive(&mut shelf, Key::X), Out::Stay, "first X only arms");
        assert!(path.exists(), "arming deletes nothing");
        assert_eq!(drive(&mut shelf, Key::X), Out::Deleted);
        assert!(!path.exists(), "the second X removes the file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_incompatible_record_refuses_to_watch_and_a_compatible_one_offers_its_path() {
        let old = entry("old", false, "/nowhere/old.json".into());
        let new = entry("new", true, "/nowhere/new.json".into());
        let mut shelf = Shelf::from_entries(vec![old, new]);
        assert_eq!(
            drive(&mut shelf, Key::Enter),
            Out::Stay,
            "an unwatchable record answers with a refusal, not a session"
        );
        drive(&mut shelf, Key::Down);
        assert_eq!(
            drive(&mut shelf, Key::Enter),
            Out::Watch("/nowhere/new.json".into())
        );
    }
}
