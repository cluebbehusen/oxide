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

    let longest = format!("{}:4200", "a".repeat(253));
    let mut screen = LobbyScreen::address(&longest);
    assert_eq!(press(&mut screen, Key::Enter), Out::Join(longest));
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
