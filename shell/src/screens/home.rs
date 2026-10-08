//! The front door: Continue, Play, Tutorial, Replays, Settings, Quit.
//! Windowless update; every row's session verb executes in the caller.

use crate::game::SoundKind;
use crate::menu::Menu;
use macroquad::prelude::Vec2;
use oxide_protocol::RawEvent;

/// What a Home frame decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Still at the door.
    Stay,
    /// Resume an interrupted recording, initially paused.
    Recover,
    /// Resume the newest autosave.
    Continue,
    /// Open the New Match wizard.
    Play,
    /// Ask for a LAN host to join.
    Join,
    /// Start the tutorial match.
    Tutorial,
    /// Open the replay shelf.
    Replays,
    /// Open the codex: every machine and works, with figures.
    Roster,
    /// Open settings.
    Settings,
    /// Leave the process.
    Quit,
}

/// The Home screen: its menu and the verb behind each row.
pub struct HomeScreen {
    /// The rows.
    pub menu: Menu,
    pub(crate) catalog_ready: bool,
    /// What each menu row does, in row order. Rows are values, not indices,
    /// so a conditional row (Recover, Continue) cannot shift its neighbours
    /// onto the wrong verb.
    rows: Vec<Out>,
    /// Independently recoverable interrupted session, if any.
    pub recovery: Option<oxide_kit::recovery::InterruptedMatch>,
}

impl HomeScreen {
    /// Builds the door, checking for a resumable autosave.
    pub fn open() -> Self {
        Self::with_resumable(false)
    }

    pub(crate) fn set_catalog(&mut self, resumable: bool, recovery: Option<std::path::PathBuf>) {
        let selected = self.rows[self.menu.selected];
        let mut fresh = Self::with_resumable(resumable).with_recovery(recovery.map(|directory| {
            oxide_kit::recovery::InterruptedMatch {
                directory,
                ticks: 0,
                scenario: String::new(),
            }
        }));
        if let Some(index) = fresh.rows.iter().position(|row| *row == selected) {
            fresh.menu.select(index);
        }
        fresh.catalog_ready = true;
        *self = fresh;
    }

    pub(crate) fn clear_recovery(&mut self) {
        self.set_catalog(self.rows.contains(&Out::Continue), None);
    }

    fn with_recovery(mut self, recovery: Option<oxide_kit::recovery::InterruptedMatch>) -> Self {
        self.recovery = recovery;
        if let Some(record) = &self.recovery {
            let seconds = record.ticks / u64::from(oxide_sim::TICKS_PER_SECOND);
            self.menu.items.insert(
                0,
                if record.ticks == 0 {
                    "Recover match".into()
                } else {
                    format!("Recover match ({:02}:{:02})", seconds / 60, seconds % 60)
                },
            );
            self.rows.insert(0, Out::Recover);
        }
        self
    }

    /// Builds the door with resumability decided by the caller (tests).
    pub fn with_resumable(resumable: bool) -> Self {
        Self::build(resumable, !crate::platform::TOUCH_ONLY)
    }

    /// Quit is left out where the platform, not the app, closes apps.
    fn build(resumable: bool, quit: bool) -> Self {
        let (items, rows) = resumable
            .then_some(("Continue", Out::Continue))
            .into_iter()
            .chain([
                ("Play", Out::Play),
                ("Join Match", Out::Join),
                ("Tutorial", Out::Tutorial),
                ("Replays", Out::Replays),
                ("Roster", Out::Roster),
                ("Settings", Out::Settings),
            ])
            .chain(quit.then_some(("Quit", Out::Quit)))
            .map(|(label, out)| (label.to_string(), out))
            .unzip();
        Self {
            menu: Menu::new("OXIDE", items),
            rows,
            recovery: None,
            catalog_ready: false,
        }
    }

    /// The standing subtitle.
    #[expect(
        clippy::unused_self,
        reason = "other screens answer this from their state"
    )]
    pub fn subtitle(&self) -> &'static str {
        "machines eating a dead world"
    }

    /// Applies a frame's events.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        let Some(choice) = self.menu.handle(events, mouse) else {
            return Out::Stay;
        };
        sounds.push((SoundKind::Click, None));
        self.rows[choice]
    }
}

#[cfg(test)]
mod tests;
