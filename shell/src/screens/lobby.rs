//! A LAN match before it starts: the host address a join asks for, then
//! the lobby's status and Cancel while machines gather. The caller
//! starts joins and installs the match once the lobby starts it.

use crate::game::{Game, SoundKind};
use crate::menu::Menu;
use crate::netplay::{Link, Lobby, with_default_port};
use crate::text_field::{Edit, TextField};
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};
use std::time::Duration;

/// Longest host address the field accepts: a MagicDNS name and a port.
const ADDRESS_MAX: usize = 64;

/// What a lobby frame decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Keep going.
    Stay,
    /// Join the host at this address, which names a port.
    Join(String),
    /// Leave for the screen underneath.
    Cancel,
}

enum Face {
    /// Asking where the host is; `failure` says why the last join did not.
    Address {
        field: TextField,
        failure: Option<String>,
    },
    /// Machines gathering.
    Waiting(Lobby),
}

/// The lobby screen.
pub struct LobbyScreen {
    face: Face,
    /// The field or the Cancel row, for drawing and the UI report.
    pub menu: Menu,
}

impl LobbyScreen {
    /// Asks for a host address, prefilled with `address`.
    pub fn address(address: &str) -> Self {
        Self::face(Face::Address {
            field: TextField::new("JOIN MATCH", "JOIN", address, ADDRESS_MAX),
            failure: None,
        })
    }

    /// Waits on `lobby`.
    pub fn waiting(lobby: Lobby) -> Self {
        Self::face(Face::Waiting(lobby))
    }

    fn face(face: Face) -> Self {
        let menu = match &face {
            Face::Address { field, .. } => field.menu(),
            Face::Waiting(_) => Menu::new("LAN MATCH", vec!["Cancel".to_owned()]),
        };
        Self { face, menu }
    }

    /// Polls the lobby; returns the match once it starts. A join that
    /// gives up comes back to the address, with the reason.
    pub fn poll(&mut self, now: Duration, viewport: Vec2) -> Option<(Game, Link)> {
        let Face::Waiting(lobby) = &mut self.face else {
            return None;
        };
        let started = lobby.poll(now, viewport);
        if let Some((address, reason)) = lobby.failure() {
            let failure = Some(reason.to_owned());
            let mut screen = Self::address(address);
            if let Face::Address { failure: slot, .. } = &mut screen.face {
                *slot = failure;
            }
            *self = screen;
        }
        started
    }

    /// Whether the address field owns input.
    pub fn text_entry(&self) -> bool {
        matches!(self.face, Face::Address { .. })
    }

    /// Consumes a request to raise the on-screen keyboard again.
    pub fn take_keyboard_request(&mut self) -> bool {
        match &mut self.face {
            Face::Address { field, .. } => field.take_keyboard_request(),
            Face::Waiting(_) => false,
        }
    }

    /// The line under the title: the lobby's status, or why the last
    /// join failed.
    pub fn subtitle(&self) -> String {
        match &self.face {
            Face::Address { failure, .. } => failure.clone().unwrap_or_default(),
            Face::Waiting(lobby) => lobby.status(),
        }
    }

    /// Applies a frame's events.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        match &mut self.face {
            Face::Address { field, .. } => match field.update(events, sounds) {
                Edit::Commit(address) => Out::Join(with_default_port(&address)),
                Edit::Cancel => Out::Cancel,
                Edit::Stay => {
                    self.menu = field.menu();
                    Out::Stay
                }
            },
            Face::Waiting(_) => {
                let escaped = events
                    .iter()
                    .any(|event| matches!(event, RawEvent::KeyDown { key: Key::Escape }));
                if self.menu.handle(events, mouse).is_some() {
                    sounds.push((SoundKind::Click, None));
                    return Out::Cancel;
                }
                if escaped { Out::Cancel } else { Out::Stay }
            }
        }
    }

    /// Draws the field or the status and Cancel.
    pub fn draw(&self, mouse: Vec2) {
        match &self.face {
            Face::Address { field, .. } => field.draw(&self.subtitle(), mouse),
            Face::Waiting(_) => self.menu.draw(&self.subtitle()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netplay::ClientLobby;
    use macroquad::prelude::vec2;

    fn feed(screen: &mut LobbyScreen, events: &[RawEvent]) -> Out {
        screen.update(events, &mut vec2(0.0, 0.0), &mut Vec::new())
    }

    fn press(screen: &mut LobbyScreen, key: Key) -> Out {
        feed(
            screen,
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        )
    }

    #[test]
    fn a_typed_host_joins_on_the_default_port_and_escape_leaves() {
        let mut screen = LobbyScreen::address("");
        assert!(screen.text_entry());
        let typed: Vec<RawEvent> = "10.0.0.2".chars().map(|ch| RawEvent::Text { ch }).collect();
        feed(&mut screen, &typed);
        assert_eq!(screen.menu.items, vec!["10.0.0.2_"]);
        assert_eq!(
            press(&mut screen, Key::Enter),
            Out::Join("10.0.0.2:4200".to_owned())
        );
        assert_eq!(press(&mut screen, Key::Escape), Out::Cancel);
    }

    #[test]
    fn a_failed_join_returns_to_its_address_with_the_reason() {
        let unreachable = "127.0.0.1:1";
        let mut screen = LobbyScreen::waiting(Lobby::Client(ClientLobby::new(unreachable, "abc")));
        assert!(!screen.text_entry());
        assert!(screen.subtitle().starts_with("Connecting to"));
        assert_eq!(screen.menu.items, vec!["Cancel"]);
        assert_eq!(press(&mut screen, Key::Escape), Out::Cancel);
        assert_eq!(press(&mut screen, Key::Enter), Out::Cancel);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !screen.text_entry() {
            assert!(
                std::time::Instant::now() < deadline,
                "the connect never failed"
            );
            assert!(screen.poll(Duration::ZERO, vec2(1280.0, 800.0)).is_none());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(screen.menu.items, vec!["127.0.0.1:1_"]);
        assert!(screen.subtitle().starts_with("Cannot reach 127.0.0.1:1"));
        assert_eq!(
            press(&mut screen, Key::Enter),
            Out::Join(unreachable.to_owned())
        );
    }
}
