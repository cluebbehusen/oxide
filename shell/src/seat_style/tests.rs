use super::*;
use crate::numeric;
use crate::numeric::Fit;
use oxide_sim::Scenario;
use oxide_sim::scenario::PlayerSpec;
use oxide_sim::scenario::ScenarioMode;

fn scenario(count: usize, team: impl Fn(usize) -> Option<u8>) -> Scenario {
    let mut map = vec![vec!['.'; 70]; 70];
    for (seat, anchor) in "12345678abcdefgh".chars().take(count).enumerate() {
        map[3 + (seat / 4) * 16][3 + (seat % 4) * 16] = anchor;
    }
    Scenario {
        mode: ScenarioMode::Match,
        name: "Seat presentation".into(),
        map: map
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: (0..count)
            .map(|seat| PlayerSpec {
                name: format!("Seat {seat}"),
                team: team(seat),
                scrap: 0,
                bot: seat != 0,
                bot_config: None,
            })
            .collect(),
        units: vec![],
        buildings: vec![],
        meta: None,
    }
}

fn rgb(color: Color) -> (u8, u8, u8) {
    (
        numeric::to_u8((color.r * 255.0).round()),
        numeric::to_u8((color.g * 255.0).round()),
        numeric::to_u8((color.b * 255.0).round()),
    )
}

#[test]
fn every_supported_ffa_seat_has_a_distinct_hostile_identity_for_every_viewer() {
    for count in 2..=MAX_PLAYERS {
        let state = scenario(count, |_| None).build().unwrap();
        for viewer in 0..count {
            for colorblind in [false, true] {
                let styles = SeatStyles::new(&state, PlayerId(viewer.fit::<u8>()), colorblind);
                let mut colors = Vec::new();
                for owner in 0..count {
                    let style = styles.get(PlayerId(owner.fit::<u8>()));
                    if owner == viewer {
                        assert_eq!(style.cue, AllegianceCue::Mine);
                        assert_eq!(style.color, roster_accent(colorblind));
                    } else {
                        assert_eq!(style.cue, AllegianceCue::Hostile);
                        let color = rgb(style.color);
                        assert!(color.0 > color.2, "hostiles stay warm");
                        assert!(
                            !colors.contains(&color),
                            "seat {owner} repeats an identity for viewer {viewer}"
                        );
                        colors.push(color);
                    }
                }
            }
        }
    }
}

#[test]
fn large_allied_team_preserves_identity_families_and_refreshes_the_viewer() {
    let scenario = scenario(MAX_PLAYERS, |seat| Some(u8::from(seat == MAX_PLAYERS - 1)));
    let state = scenario.build().unwrap();
    for colorblind in [false, true] {
        let allied = SeatStyles::new(&state, PlayerId(0), colorblind);
        let enemy = SeatStyles::new(&state, PlayerId(15), colorblind);
        let mut colors = Vec::new();
        for owner in 1..15 {
            let style = allied.get(PlayerId(owner));
            assert_eq!(style.cue, AllegianceCue::Ally);
            assert!(!colors.contains(&rgb(style.color)));
            colors.push(rgb(style.color));
            if !colorblind {
                assert!(style.color.b > style.color.r);
            }
            assert_eq!(enemy.get(PlayerId(owner)).cue, AllegianceCue::Hostile);
        }
        assert_eq!(enemy.get(PlayerId(15)).cue, AllegianceCue::Mine);
        let first_ally = allied.get(PlayerId(1)).color;
        let first_enemy = enemy.get(PlayerId(0)).color;
        if colorblind {
            let lum = |c: Color| 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
            assert!(lum(first_ally) - lum(first_enemy) > 0.3);
        } else {
            assert_eq!(rgb(first_ally), (100, 160, 245));
            assert_eq!(rgb(first_enemy), (228, 44, 58));
        }
    }
}

#[test]
fn live_and_replay_views_share_prepared_styles_after_viewer_changes() {
    let scenario = scenario(MAX_PLAYERS, |seat| Some((seat / 8).fit::<u8>()));
    let viewport = macroquad::prelude::vec2(1100.0, 720.0);
    let mut game = crate::game::Game::with_viewport(scenario.clone(), viewport).unwrap();
    let mut playback = crate::screens::playback::PlaybackSession::from_replay(
        oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, "test", scenario),
    )
    .unwrap();
    for viewer in [0, 8, 15] {
        game.presentation.human = PlayerId(viewer);
        playback.presentation.human = PlayerId(viewer);
        let live = game.view();
        let replay = playback.view();
        for owner in 0..16 {
            let owner = PlayerId(owner);
            assert_eq!(
                crate::render::seat_identity_color(&live, owner),
                crate::render::seat_identity_color(&replay, owner)
            );
            assert_eq!(
                crate::render::seat_identity_tint(&live, owner).is_none(),
                owner == PlayerId(viewer)
            );
        }
    }
}
