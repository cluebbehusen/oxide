//! The screens and what each one is: every per-screen fact the frame loop,
//! the debug socket and the audio frame ask about lives here, answered by
//! one exhaustive match apiece so a new screen cannot fall into a default.

use super::persistence;
use crate::game::{Game, Presentation};
use crate::screens::codex::CodexScreen;
use crate::screens::final_map::FinalMapScreen;
use crate::screens::home::HomeScreen;
use crate::screens::lobby::LobbyScreen;
use crate::screens::pause::PauseScreen;
use crate::screens::playback::PlaybackSession;
use crate::screens::results::ResultsScreen;
use crate::screens::settings::SettingsScreen;
use crate::screens::shelf::Shelf;
use crate::screens::wizard::Wizard;

/// Which screen owns input this frame, holding that screen's state.
/// Match choices live in the [`crate::screens::wizard::NewMatchDraft`] on
/// [`super::App`], not in a screen.
pub(super) enum Screen {
    /// The front door: play, settings, quit.
    Home(HomeScreen),
    /// Settings and the Controls remap screen. `back` is where leaving
    /// returns to — Home, or the untouched pause menu whose payload
    /// waits here intact so the cursor comes back to the row that
    /// opened this screen.
    Settings {
        /// The screen itself.
        screen: SettingsScreen,
        /// The displaced screen, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The codex of machines and works, opened over Home or the paused
    /// match, which waits in `back` as it does under Settings.
    Codex {
        /// The screen itself.
        screen: CodexScreen,
        /// The displaced screen, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The New Match wizard (map grid, then match setup).
    Wizard(Wizard),
    /// A LAN match gathering its machines. Leaving drops every
    /// connection the lobby holds.
    Lobby {
        /// The screen itself.
        screen: Box<LobbyScreen>,
        /// Where leaving returns to: Home, or the match setup that hosted.
        back: Box<Screen>,
    },
    /// The game proper. The session lives in [`super::App::game`], which
    /// every screen needs as its backdrop.
    Playing,
    /// Read-only replay playback: the log is the match, seek included.
    Playback {
        /// Boxed: the session carries a whole presentation, and the enum
        /// should not make every other screen pay its size.
        session: Box<PlaybackSession>,
        /// The screen that opened the viewer, restored wholesale on leave.
        back: Box<Screen>,
    },
    /// The replay shelf.
    Replays(Shelf),
    /// Decided-match report and next steps.
    Results(ResultsScreen),
    /// Camera-only inspection of the already-final live state.
    FinalMap(FinalMapScreen),
    /// Game visible but veiled; the pause screen owns input,
    /// confirmation and save-naming state included.
    Pause(PauseScreen),
    /// A save or load in flight over the screen it displaced.
    Busy(Box<persistence::Busy>),
}

/// A screen's variant without its state, for comparing screens across a
/// frame and for the facts that depend on the variant alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScreenKind {
    Home,
    Settings,
    Codex,
    Wizard,
    Lobby,
    Playing,
    Playback,
    Replays,
    Results,
    FinalMap,
    Pause,
    Busy,
}

impl ScreenKind {
    /// Whether the world is the screen rather than a backdrop: gameplay input
    /// reaches it, and menu notices wait for a menu.
    pub(super) const fn gameplay(self) -> bool {
        match self {
            Self::Playing | Self::Playback | Self::FinalMap => true,
            Self::Home
            | Self::Settings
            | Self::Codex
            | Self::Wizard
            | Self::Lobby
            | Self::Replays
            | Self::Results
            | Self::Pause
            | Self::Busy => false,
        }
    }

    /// The performance display's context; zero turns it off.
    pub(super) const fn performance_context(self) -> u8 {
        match self {
            Self::Playing => 1,
            Self::Playback => 2,
            Self::FinalMap => 3,
            Self::Home
            | Self::Settings
            | Self::Codex
            | Self::Wizard
            | Self::Lobby
            | Self::Replays
            | Self::Results
            | Self::Pause
            | Self::Busy => 0,
        }
    }
}

/// The session a screen shows: the live match, or a replay viewer's own.
#[derive(Clone, Copy)]
pub(super) enum Visible<'a> {
    Live(&'a Game),
    Replay(&'a PlaybackSession),
}

impl<'a> Visible<'a> {
    pub(super) fn state(self) -> &'a oxide_sim::State {
        match self {
            Self::Live(game) => &game.state,
            Self::Replay(session) => &session.engine.state,
        }
    }

    pub(super) fn presentation(self) -> &'a Presentation {
        match self {
            Self::Live(game) => &game.presentation,
            Self::Replay(session) => &session.presentation,
        }
    }

    /// The tick on screen.
    pub(super) fn tick(self) -> u64 {
        match self {
            Self::Live(game) => game.state.current_tick(),
            Self::Replay(session) => session.engine.position(),
        }
    }

    pub(super) fn speed(self) -> f64 {
        match self {
            Self::Live(game) => game.clock.speed,
            Self::Replay(session) => session.clock.speed,
        }
    }

    pub(super) fn paused(self) -> bool {
        match self {
            Self::Live(game) => game.clock.paused,
            Self::Replay(session) => session.clock.paused,
        }
    }

    /// The crash-recovery recording behind the session; a replay has none.
    pub(super) fn recording(
        self,
    ) -> Option<&'a std::sync::Arc<oxide_kit::recovery::RecoveryWriter>> {
        match self {
            Self::Live(game) => game.recovery.as_ref(),
            Self::Replay(_) => None,
        }
    }
}

impl Screen {
    pub(super) fn kind(&self) -> ScreenKind {
        match self {
            Self::Home(_) => ScreenKind::Home,
            Self::Settings { .. } => ScreenKind::Settings,
            Self::Codex { .. } => ScreenKind::Codex,
            Self::Wizard(_) => ScreenKind::Wizard,
            Self::Lobby { .. } => ScreenKind::Lobby,
            Self::Playing => ScreenKind::Playing,
            Self::Playback { .. } => ScreenKind::Playback,
            Self::Replays(_) => ScreenKind::Replays,
            Self::Results(_) => ScreenKind::Results,
            Self::FinalMap(_) => ScreenKind::FinalMap,
            Self::Pause(_) => ScreenKind::Pause,
            Self::Busy(_) => ScreenKind::Busy,
        }
    }

    /// The session on screen: a replay viewer shows its own, every other
    /// screen shows the live match, as play or as backdrop.
    pub(super) fn visible<'a>(&'a self, live: &'a Game) -> Visible<'a> {
        match self {
            Self::Playback { session, .. } => Visible::Replay(session),
            _ => Visible::Live(live),
        }
    }

    /// The presentation on screen, for requests that steer what the
    /// window shows.
    pub(super) fn visible_presentation<'a>(
        &'a mut self,
        live: &'a mut Game,
    ) -> &'a mut Presentation {
        match self {
            Self::Playback { session, .. } => &mut session.presentation,
            _ => &mut live.presentation,
        }
    }

    /// The session the debug protocol's shared verbs act on.
    pub(super) fn visible_session<'a>(
        &'a mut self,
        live: &'a mut Game,
    ) -> &'a mut dyn oxide_protocol::DebugSession {
        match self {
            Self::Playback { session, .. } => &mut **session,
            _ => live,
        }
    }

    /// Whether this is the pause menu, or a menu it opened that returns to it.
    pub(super) fn over_pause(&self) -> bool {
        match self {
            Self::Pause(_) => true,
            Self::Settings { back, .. } | Self::Codex { back, .. } => back.over_pause(),
            _ => false,
        }
    }

    /// Whether closing the window would abandon a live match behind this
    /// screen. This decides whether Cancel on a failed quit-save can return
    /// to that match or must return to the front door, and whether a LAN
    /// match keeps its link. A save or load in flight keeps the match.
    pub(super) fn holds_live_match(&self) -> bool {
        match self {
            Self::Playing
            | Self::Results(_)
            | Self::FinalMap(_)
            | Self::Pause(_)
            | Self::Busy(_) => true,
            Self::Settings { .. } | Self::Codex { .. } => self.over_pause(),
            Self::Playback { back, .. } => back.holds_live_match(),
            Self::Home(_) | Self::Wizard(_) | Self::Replays(_) | Self::Lobby { .. } => false,
        }
    }

    /// Whether this screen owns a decorative backdrop whose presentation
    /// clock should keep moving. A menu opened from Pause is deliberately
    /// different from the same menu opened from Home: the paused battlefield
    /// stays frozen behind it, as it does behind a save in flight.
    pub(super) fn backdrop_runs(&self) -> bool {
        match self {
            Self::Home(_)
            | Self::Wizard(_)
            | Self::Replays(_)
            | Self::Results(_)
            | Self::Lobby { .. } => true,
            Self::Settings { .. } | Self::Codex { .. } => !self.over_pause(),
            Self::Playing
            | Self::Playback { .. }
            | Self::FinalMap(_)
            | Self::Pause(_)
            | Self::Busy(_) => false,
        }
    }

    /// Whether a text field owns input: the save-name field or the host
    /// address.
    pub(super) fn text_entry(&self) -> bool {
        match self {
            Self::Pause(pause) => pause.naming(),
            Self::Lobby { screen, .. } => screen.text_entry(),
            Self::Home(_)
            | Self::Settings { .. }
            | Self::Codex { .. }
            | Self::Wizard(_)
            | Self::Playing
            | Self::Playback { .. }
            | Self::Replays(_)
            | Self::Results(_)
            | Self::FinalMap(_)
            | Self::Busy(_) => false,
        }
    }

    /// Whether the text field on screen asked to bring back the on-screen
    /// keyboard this frame.
    pub(super) fn take_keyboard_request(&mut self) -> bool {
        match self {
            Self::Pause(pause) => pause.take_keyboard_request(),
            Self::Lobby { screen, .. } => screen.take_keyboard_request(),
            Self::Home(_)
            | Self::Settings { .. }
            | Self::Codex { .. }
            | Self::Wizard(_)
            | Self::Playing
            | Self::Playback { .. }
            | Self::Replays(_)
            | Self::Results(_)
            | Self::FinalMap(_)
            | Self::Busy(_) => false,
        }
    }

    /// The diagnostics' coarse name for the screen.
    pub(super) fn profile_mode(&self) -> &'static str {
        match self {
            Self::Home(_) => "home",
            Self::Settings { .. } => "settings",
            Self::Codex { .. } => "codex",
            Self::Wizard(_) => "wizard",
            Self::Lobby { .. } => "lobby",
            Self::Playing => "playing",
            Self::Playback { .. } => "playback",
            Self::FinalMap(_) => "final_map",
            Self::Results(_) => "results",
            Self::Replays(_) => "replays",
            Self::Pause(_) => "pause",
            Self::Busy(busy) => busy.mode(),
        }
    }

    /// The debug protocol's stable name for what the player is looking at,
    /// which also keys the coaching clock.
    pub(super) fn mode(&self) -> &'static str {
        match self {
            Self::Home(_) => "home",
            Self::Settings { screen, .. } => screen.mode_name(),
            Self::Codex { screen, .. } => screen.mode_name(),
            Self::Wizard(wizard) => wizard.mode_name(),
            Self::Playing => "playing",
            Self::Playback { .. } => "playback",
            Self::FinalMap(_) => "final_map",
            Self::Results(_) => "results",
            Self::Replays(_) => "replays",
            Self::Lobby { .. } => "lobby",
            Self::Busy(busy) => busy.mode(),
            Self::Pause(pause) => {
                if pause.saving_failed() {
                    "save_failed"
                } else if pause.naming() {
                    "save_name"
                } else if pause.confirming() {
                    "confirm_pause"
                } else {
                    "pause_menu"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
