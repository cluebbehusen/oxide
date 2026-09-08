//! Launch poses retained only while their authoritative payloads are in flight.

use std::collections::HashMap;

use macroquad::prelude::{Vec2, vec2};
use oxide_sim::state::Shell;
use oxide_sim::{Event, ProjectileKind, State, Target, UnitId, UnitKind};

#[derive(Debug, Default)]
pub(crate) struct ProjectileReleases {
    releases: HashMap<(UnitId, u64), Vec<LaunchPose>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LaunchPose {
    pub(crate) heading: Vec2,
    pub(crate) kind: UnitKind,
    pub(crate) slot: usize,
}

impl ProjectileReleases {
    pub(crate) fn observe(&mut self, state: &State, events: &[Event]) {
        self.releases
            .retain(|(_, arrival), _| *arrival >= state.current_tick());
        let mut slots = HashMap::<UnitId, usize>::new();
        for event in events {
            if let Event::ShellLaunched {
                shooter: Target::Unit(id),
                flight,
                ..
            } = event
                && let Some(unit) = state.unit(*id)
                && matches!(
                    unit.kind,
                    UnitKind::Condor | UnitKind::Moth | UnitKind::Bombard
                )
            {
                let heading = chassis::compass::dir(unit.heading);
                let slot = slots.entry(*id).or_default();
                self.releases
                    .entry((*id, state.current_tick() - 1 + flight))
                    .or_default()
                    .push(LaunchPose {
                        heading: vec2(heading.x.to_num::<f32>(), heading.y.to_num::<f32>()),
                        kind: unit.kind,
                        slot: *slot,
                    });
                *slot += 1;
            }
        }
    }

    pub(crate) fn artillery_heading(&self, shell: &Shell) -> Option<Vec2> {
        let Target::Unit(id) = shell.shooter else {
            return None;
        };
        self.releases
            .get(&(id, shell.arrival))?
            .first()
            .filter(|pose| pose.kind == UnitKind::Bombard)
            .map(|pose| pose.heading)
    }

    pub(crate) fn release(&self, shells: &[Shell], index: usize) -> Option<LaunchPose> {
        let shell = shells.get(index)?;
        if shell.kind != ProjectileKind::Bomb {
            return None;
        }
        let Target::Unit(id) = shell.shooter else {
            return None;
        };
        let releases = self.releases.get(&(id, shell.arrival))?;
        // Map-edge clamping can give several bombs the same arrival tick.
        // Those shells keep launch order and are removed together.
        let occurrence = if releases.len() == 1 {
            0
        } else {
            shells[..index]
                .iter()
                .filter(|earlier| {
                    earlier.shooter == shell.shooter && earlier.arrival == shell.arrival
                })
                .count()
        };
        releases.get(occurrence).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use chassis::grid::TilePos;
    use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario};

    #[test]
    fn saved_projectile_releases_retain_heading_slots_and_simulation_parity() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "name": "Bomber release", "seed": 619,
            "map": [
                "....................................",
                ".1............................2.....",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "....................................",
                "...................................."
            ],
            "players": [
                {"name":"You", "faction":"ferrous", "scrap":0, "bot":false},
                {"name":"Target", "faction":"cupric", "scrap":0, "bot":true}
            ],
            "units": [
                {"player":0, "kind":"condor", "x":16, "y":8},
                {"player":0, "kind":"moth", "x":16, "y":12}
            ],
            "buildings": [
                {"player":1, "kind":"repair_bay", "x":25, "y":7},
                {"player":1, "kind":"repair_bay", "x":25, "y":11}
            ]
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, vec2(1440.0, 900.0)).unwrap();
        for (unit, y) in [(0, 8), (1, 12)] {
            game.pending.push(PlayerCommand {
                player: PlayerId(0),
                command: Command::AttackMove {
                    units: vec![UnitId(unit)],
                    goal: TilePos::new(27, y),
                    queue: false,
                },
            });
        }
        let mut saw_moth = false;
        let mut condor = None;
        for _ in 0..240 {
            let report = game.do_tick();
            let shells = game.state.shells();
            for (index, shell) in shells.iter().enumerate() {
                let pose = game.projectile_releases.release(shells, index).unwrap();
                if shell.shooter == Target::Unit(UnitId(1)) {
                    assert_eq!(pose.kind, UnitKind::Moth);
                    if !saw_moth {
                        let moth: Vec<_> = shells
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s.shooter == shell.shooter)
                            .map(|(i, _)| game.projectile_releases.release(shells, i).unwrap())
                            .collect();
                        assert_eq!(
                            moth.iter().map(|p| p.slot).collect::<Vec<_>>(),
                            vec![0, 1, 2, 3, 4, 5]
                        );
                        let mut replay = game.recorder.clone();
                        replay.meta.ticks = Some(game.state.current_tick());
                        let loaded = Game::from_replay(replay).unwrap();
                        assert_eq!(loaded.hash_hex(), game.hash_hex());
                        for i in 0..shells.len() {
                            assert_eq!(
                                loaded.projectile_releases.release(loaded.state.shells(), i),
                                game.projectile_releases.release(shells, i)
                            );
                        }
                        let mut clamped_events = report.events.clone();
                        for event in &mut clamped_events {
                            if let Event::ShellLaunched { flight, .. } = event {
                                *flight = 1;
                            }
                        }
                        let mut clamped = shells.to_vec();
                        for shell in &mut clamped {
                            shell.arrival = game.state.current_tick();
                        }
                        let mut releases = ProjectileReleases::default();
                        releases.observe(&game.state, &clamped_events);
                        let slots: Vec<_> = clamped
                            .iter()
                            .enumerate()
                            .map(|(i, _)| releases.release(&clamped, i).unwrap().slot)
                            .collect();
                        assert_eq!(slots, vec![0, 1, 2, 3, 4, 5]);
                        saw_moth = true;
                    }
                } else if shell.shooter == Target::Unit(UnitId(0)) {
                    assert_eq!(pose.kind, UnitKind::Condor);
                    assert_eq!(pose.slot, 0);
                    condor = Some((shell.clone(), pose));
                }
            }
            if condor.is_some() {
                break;
            }
        }
        assert!(saw_moth);
        let (shell, pose) = condor.expect("Condor completes its approach and releases");
        let mut replay = game.recorder.clone();
        replay.meta.ticks = Some(game.state.current_tick());
        let mut resumed = Game::from_replay(replay).unwrap();
        assert_eq!(resumed.hash_hex(), game.hash_hex());
        let lookup = |game: &Game| {
            let index = game.state.shells().iter().position(|s| s == &shell)?;
            game.projectile_releases.release(game.state.shells(), index)
        };
        assert_eq!(lookup(&resumed), Some(pose));
        game.advance_ticks(2);
        assert_eq!(lookup(&game), Some(pose));
        resumed.advance_ticks(2);
        assert_eq!(resumed.hash_hex(), game.hash_hex());
        game.advance_ticks(60);
        assert_eq!(lookup(&game), None);
        resumed.replace_state_after_jump(&game.state);
        assert_eq!(lookup(&resumed), None);
    }
}
