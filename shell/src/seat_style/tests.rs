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
                    let seat = PlayerId(owner.fit::<u8>());
                    let style = styles.get(seat);
                    let cue = AllegianceCue::of(&state, PlayerId(viewer.fit::<u8>()), seat);
                    if owner == viewer {
                        assert_eq!(cue, AllegianceCue::Mine);
                        assert_eq!(style, self_color(colorblind));
                    } else {
                        assert_eq!(cue, AllegianceCue::Hostile);
                        let color = rgb(style);
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
            let color = allied.get(PlayerId(owner));
            assert_eq!(
                AllegianceCue::of(&state, PlayerId(0), PlayerId(owner)),
                AllegianceCue::Ally
            );
            assert!(!colors.contains(&rgb(color)));
            colors.push(rgb(color));
            assert!(color.b > color.r, "allies stay cool");
            assert_eq!(
                AllegianceCue::of(&state, PlayerId(15), PlayerId(owner)),
                AllegianceCue::Hostile
            );
        }
        assert_eq!(enemy.get(PlayerId(15)), self_color(colorblind));
        if !colorblind {
            assert_eq!(rgb(allied.get(PlayerId(1))), (100, 160, 245));
            assert_eq!(rgb(enemy.get(PlayerId(0))), (228, 44, 58));
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
                crate::render::seat_identity_color(&live, owner) == crate::render::self_color(),
                owner == PlayerId(viewer)
            );
        }
    }
}

fn hue(color: Color) -> f32 {
    let (r, g, b) = (color.r, color.g, color.b);
    let max = r.max(g).max(b);
    let chroma = max - r.min(g).min(b);
    let sector = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    sector * 60.0
}

fn chroma(color: Color) -> f32 {
    color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b)
}

#[test]
fn the_self_color_never_reads_as_a_friend_or_a_foe() {
    let mine = self_color(false);
    for color in ally_palette(false)
        .into_iter()
        .chain(hostile_palette(false))
    {
        let apart = (hue(mine) - hue(color)).abs();
        assert!(
            apart.min(360.0 - apart) >= 30.0,
            "{:?} sits within 30 degrees of the self color",
            rgb(color)
        );
    }
    // Colorblind play separates by saturation instead of hue: an
    // achromatic self color stays white under every color-vision type,
    // while both families keep strong color.
    let mine = self_color(true);
    assert!(chroma(mine) < 0.05);
    let lum = |c: Color| 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
    for color in ally_palette(true).into_iter().chain(hostile_palette(true)) {
        assert!(chroma(color) > 0.3, "{:?} is too gray", rgb(color));
        assert!(
            lum(mine) > lum(color),
            "{:?} outshines the self color",
            rgb(color)
        );
    }
}
