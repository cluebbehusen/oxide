//! Viewer-relative presentation identity, prepared once for a live or replay view.
use macroquad::prelude::{Color, WHITE, color_u8};
use oxide_sim::scenario::MAX_PLAYERS;
use oxide_sim::{PlayerId, State};

/// Relationship to the viewer, independent of faction artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllegianceCue {
    Mine,
    Ally,
    Hostile,
}

#[derive(Clone, Copy)]
pub(crate) struct SeatStyle {
    pub cue: AllegianceCue,
    pub color: Color,
}

#[derive(Clone, Copy)]
pub(crate) struct SeatStyles([SeatStyle; MAX_PLAYERS]);

impl SeatStyles {
    pub fn new(state: &State, viewer: PlayerId, colorblind: bool) -> Self {
        let mut allies = 0;
        let mut hostiles = 0;
        let mut styles = [SeatStyle {
            cue: AllegianceCue::Mine,
            color: WHITE,
        }; MAX_PLAYERS];
        for (seat, player) in state.players().iter().enumerate() {
            let owner = PlayerId(seat as u8);
            let cue = if owner == viewer {
                AllegianceCue::Mine
            } else if !state.hostile(viewer, owner) {
                AllegianceCue::Ally
            } else {
                AllegianceCue::Hostile
            };
            let color = match cue {
                AllegianceCue::Mine => faction_accent(player.faction, colorblind),
                AllegianceCue::Ally => {
                    let color = identity_color(cue, allies, colorblind);
                    allies += 1;
                    color
                }
                AllegianceCue::Hostile => {
                    let color = identity_color(cue, hostiles, colorblind);
                    hostiles += 1;
                    color
                }
            };
            styles[seat] = SeatStyle { cue, color };
        }
        Self(styles)
    }

    pub fn get(&self, owner: PlayerId) -> SeatStyle {
        self.0[usize::from(owner.0)]
    }
}

pub(crate) fn faction_accent(faction: oxide_sim::Faction, colorblind: bool) -> Color {
    match (faction, colorblind) {
        (oxide_sim::Faction::Ferrous, false) => color_u8!(196, 87, 59, 255),
        (oxide_sim::Faction::Cupric, false) => color_u8!(63, 148, 130, 255),
        // The safe pair: warm orange vs cool blue reads under deutan,
        // protan, and tritan alike.
        (oxide_sim::Faction::Ferrous, true) => color_u8!(230, 120, 30, 255),
        (oxide_sim::Faction::Cupric, true) => color_u8!(70, 120, 235, 255),
    }
}

// Hue supplements the allegiance rings and labels; it is not the sole identity cue.
fn identity_color(cue: AllegianceCue, rank: usize, colorblind: bool) -> Color {
    let allies = if colorblind {
        [
            color_u8!(238, 234, 222, 255),
            color_u8!(112, 184, 238, 255),
            color_u8!(181, 158, 232, 255),
            color_u8!(145, 207, 190, 255),
            color_u8!(107, 148, 224, 255),
            color_u8!(137, 207, 229, 255),
            color_u8!(200, 181, 239, 255),
            color_u8!(111, 188, 174, 255),
        ]
    } else {
        [
            color_u8!(100, 160, 245, 255),
            color_u8!(68, 190, 205, 255),
            color_u8!(165, 139, 235, 255),
            color_u8!(132, 201, 170, 255),
            color_u8!(64, 119, 221, 255),
            color_u8!(113, 203, 239, 255),
            color_u8!(196, 162, 242, 255),
            color_u8!(72, 174, 157, 255),
        ]
    };
    let hostiles = if colorblind {
        [
            color_u8!(211, 65, 60, 255),
            color_u8!(232, 128, 35, 255),
            color_u8!(173, 72, 125, 255),
            color_u8!(151, 87, 61, 255),
            color_u8!(242, 84, 31, 255),
            color_u8!(207, 99, 104, 255),
            color_u8!(190, 136, 43, 255),
            color_u8!(139, 49, 82, 255),
        ]
    } else {
        [
            color_u8!(228, 44, 58, 255),
            color_u8!(232, 105, 42, 255),
            color_u8!(199, 66, 132, 255),
            color_u8!(172, 83, 57, 255),
            color_u8!(246, 73, 24, 255),
            color_u8!(210, 85, 111, 255),
            color_u8!(196, 124, 37, 255),
            color_u8!(154, 50, 85, 255),
        ]
    };
    let palette = match cue {
        AllegianceCue::Ally => allies,
        AllegianceCue::Hostile => hostiles,
        AllegianceCue::Mine => unreachable!("own seats use their faction accent"),
    };
    let base = palette[rank % palette.len()];
    if rank < palette.len() {
        base
    } else {
        Color::new(
            base.r * 0.7 + 0.3,
            base.g * 0.7 + 0.3,
            base.b * 0.7 + 0.3,
            1.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::PlayerSpec;
    use oxide_sim::{Faction, Scenario};

    fn scenario(count: usize, team: impl Fn(usize) -> Option<u8>) -> Scenario {
        let mut map = vec![vec!['.'; 70]; 70];
        for (seat, anchor) in "12345678abcdefgh".chars().take(count).enumerate() {
            map[3 + (seat / 4) * 16][3 + (seat % 4) * 16] = anchor;
        }
        Scenario {
            name: "Seat presentation".into(),
            seed: 1,
            map: map
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: (0..count)
                .map(|seat| PlayerSpec {
                    name: format!("Seat {seat}"),
                    faction: if seat % 2 == 0 {
                        Faction::Ferrous
                    } else {
                        Faction::Cupric
                    },
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
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8,
        )
    }

    #[test]
    fn every_supported_ffa_seat_has_a_distinct_hostile_identity_for_every_viewer() {
        for count in 2..=MAX_PLAYERS {
            let state = scenario(count, |_| None).build().unwrap();
            for viewer in 0..count {
                for colorblind in [false, true] {
                    let styles = SeatStyles::new(&state, PlayerId(viewer as u8), colorblind);
                    let mut colors = Vec::new();
                    for owner in 0..count {
                        let style = styles.get(PlayerId(owner as u8));
                        if owner == viewer {
                            assert_eq!(style.cue, AllegianceCue::Mine);
                            assert_eq!(
                                style.color,
                                faction_accent(state.players()[owner].faction, colorblind)
                            );
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
        let scenario = scenario(MAX_PLAYERS, |seat| Some((seat / 8) as u8));
        let viewport = macroquad::prelude::vec2(1100.0, 720.0);
        let mut game = crate::game::Game::with_viewport(scenario.clone(), viewport).unwrap();
        let mut playback = crate::screens::playback::PlaybackSession::from_replay(
            oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, scenario),
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
}
