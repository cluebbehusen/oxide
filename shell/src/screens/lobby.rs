//! Waiting for a LAN match to start: the lobby's status, Cancel, and Retry
//! after a failed join. The caller polls the lobby and installs the match.

use crate::game::SoundKind;
use crate::menu::Menu;
use crate::netplay::Lobby;
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

/// What a lobby frame decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Keep waiting.
    Stay,
    /// Try joining again.
    Retry,
    /// Leave for Home.
    Cancel,
}

/// The lobby screen: the lobby itself and its rows.
pub struct LobbyScreen {
    /// The machines gathering for the match.
    pub lobby: Lobby,
    /// The rows.
    pub menu: Menu,
    rows: Vec<Out>,
}

impl LobbyScreen {
    /// Opens the screen over `lobby`.
    pub fn open(lobby: Lobby) -> Self {
        let mut screen = Self {
            lobby,
            menu: Menu::new("LAN MATCH", Vec::new()),
            rows: Vec::new(),
        };
        screen.refresh();
        screen
    }

    /// Offers Retry only while a join has failed.
    fn refresh(&mut self) {
        let rows = if self.lobby.failed() {
            vec![Out::Retry, Out::Cancel]
        } else {
            vec![Out::Cancel]
        };
        if rows != self.rows {
            self.menu = Menu::new(
                "LAN MATCH",
                rows.iter()
                    .map(|row| match row {
                        Out::Retry => "Retry".to_owned(),
                        _ => "Cancel".to_owned(),
                    })
                    .collect(),
            );
            self.rows = rows;
        }
    }

    /// Applies a frame's events.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        self.refresh();
        if events
            .iter()
            .any(|event| matches!(event, RawEvent::KeyDown { key: Key::Escape }))
        {
            return Out::Cancel;
        }
        let Some(choice) = self.menu.handle(events, mouse) else {
            return Out::Stay;
        };
        sounds.push((SoundKind::Click, None));
        let out = self.rows[choice];
        if out == Out::Retry {
            self.lobby.retry();
            self.refresh();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netplay::ClientLobby;
    use macroquad::prelude::vec2;

    fn press(screen: &mut LobbyScreen, key: Key) -> Out {
        screen.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            &mut vec2(0.0, 0.0),
            &mut Vec::new(),
        )
    }

    #[test]
    fn escape_or_cancel_leaves_and_retry_follows_a_failed_join() {
        let unreachable = "127.0.0.1:1";
        let mut screen = LobbyScreen::open(Lobby::Client(ClientLobby::new(unreachable, "abc")));
        assert_eq!(screen.menu.items, vec!["Cancel"]);
        assert_eq!(press(&mut screen, Key::Escape), Out::Cancel);
        assert_eq!(press(&mut screen, Key::Enter), Out::Cancel);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !screen.lobby.failed() {
            assert!(
                std::time::Instant::now() < deadline,
                "the connect never failed"
            );
            screen
                .lobby
                .poll(std::time::Duration::ZERO, vec2(1280.0, 800.0));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(press(&mut screen, Key::Enter), Out::Retry);
        assert!(screen.lobby.status().starts_with("Connecting to"));
        assert_eq!(screen.menu.items, vec!["Cancel"]);
    }
}
