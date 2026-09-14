//! The front door: Continue, Play, Tutorial, Replays, Settings, Quit.
//! Windowless update; every row's session verb executes in the caller.

use crate::autosave;
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

/// The Home screen: its menu and whether a Continue row leads it.
pub struct HomeScreen {
    /// The rows.
    pub menu: Menu,
    /// Whether row zero is Continue (a compatible autosave exists).
    pub resumable: bool,
    /// Independently recoverable interrupted session, if any.
    pub recovery: Option<oxide_kit::recovery::InterruptedMatch>,
}

impl HomeScreen {
    /// Builds the door, checking for a resumable autosave.
    pub fn open() -> Self {
        let resumable = autosave::latest_compatible().is_some();
        Self::with_resumable(resumable).with_recovery(
            crate::paths::recovery_dir()
                .and_then(|root| oxide_kit::recovery::latest_interrupted(&root)),
        )
    }

    fn with_recovery(mut self, recovery: Option<oxide_kit::recovery::InterruptedMatch>) -> Self {
        self.recovery = recovery;
        if let Some(record) = &self.recovery {
            let seconds = record.ticks / oxide_sim::TICKS_PER_SECOND as u64;
            self.menu.items.insert(
                0,
                format!("Recover match ({:02}:{:02})", seconds / 60, seconds % 60),
            );
        }
        self
    }

    /// Builds the door with resumability decided by the caller (tests).
    pub fn with_resumable(resumable: bool) -> Self {
        let mut items = Vec::new();
        if resumable {
            items.push("Continue".to_string());
        }
        items.extend(
            ["Play", "Tutorial", "Replays", "Roster", "Settings", "Quit"].map(str::to_string),
        );
        Self {
            menu: Menu::new("OXIDE", items),
            resumable,
            recovery: None,
        }
    }

    /// The standing subtitle.
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
        if self.recovery.is_some() && choice == 0 {
            return Out::Recover;
        }
        let choice = choice - usize::from(self.recovery.is_some());
        let base = if self.resumable { choice } else { choice + 1 };
        match base {
            0 => Out::Continue,
            1 => Out::Play,
            2 => Out::Tutorial,
            3 => Out::Replays,
            4 => Out::Roster,
            5 => Out::Settings,
            _ => Out::Quit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_protocol::Key;

    fn pick(home: &mut HomeScreen, row: usize) -> Out {
        home.menu.select(row);
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        home.update(
            &[
                RawEvent::KeyDown { key: Key::Enter },
                RawEvent::KeyUp { key: Key::Enter },
            ],
            &mut mouse,
            &mut sounds,
        )
    }

    #[test]
    fn rows_mean_the_same_verbs_with_and_without_a_continue_row() {
        // The row shift is where a blind index goes wrong (the battery
        // once resumed a match instead of opening the map list).
        let mut fresh = HomeScreen::with_resumable(false);
        assert_eq!(pick(&mut fresh, 0), Out::Play);
        assert_eq!(pick(&mut fresh, 2), Out::Replays);
        assert_eq!(pick(&mut fresh, 3), Out::Roster);
        assert_eq!(pick(&mut fresh, 4), Out::Settings);
        assert_eq!(pick(&mut fresh, 5), Out::Quit);

        let mut resumable = HomeScreen::with_resumable(true);
        assert_eq!(pick(&mut resumable, 0), Out::Continue);
        assert_eq!(pick(&mut resumable, 1), Out::Play);
        assert_eq!(pick(&mut resumable, 4), Out::Roster);
        assert_eq!(pick(&mut resumable, 6), Out::Quit);
    }
    #[test]
    fn recovery_is_explicit_and_does_not_change_continue_or_play() {
        for resumable in [false, true] {
            let mut home = HomeScreen::with_resumable(resumable).with_recovery(Some(
                oxide_kit::recovery::InterruptedMatch {
                    directory: "/unused".into(),
                    ticks: 2400,
                    scenario: "test".into(),
                },
            ));
            assert!(home.menu.items[0].contains("02:00"));
            assert_eq!(pick(&mut home, 0), Out::Recover);
            assert_eq!(
                pick(&mut home, 1),
                if resumable { Out::Continue } else { Out::Play }
            );
            assert_eq!(pick(&mut home, usize::from(resumable) + 5), Out::Settings);
        }
    }
}
