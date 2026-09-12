//! Motor loops driven by the currently presented missile state.

use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;
use macroquad::audio::{PlaySoundParams, Sound, play_sound, set_sound_volume, stop_sound};
use macroquad::prelude::Vec2;
use oxide_sim::{ProjectileKind, Target};

use crate::audio_timeline::{missile_ejection_ticks, missile_elapsed_ticks};
use crate::game::{Game, SoundKind, world_vec};

pub(crate) const MOTOR_VOICES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RocketKey {
    shooter: Target,
    launch: Vec2Fx,
    impact: Vec2Fx,
    arrival: u64,
    occurrence: usize,
}

#[derive(Clone, Copy, Debug)]
struct Motor {
    key: RocketKey,
    gain: f32,
}

fn motor_envelope(elapsed: f32, total: f32) -> f32 {
    let powered = elapsed - missile_ejection_ticks(total);
    if powered < 0.0 || elapsed >= total {
        return 0.0;
    }
    // A short fade at both ends prevents discontinuities at loop admission.
    (powered / 0.4).clamp(0.0, 1.0) * ((total - elapsed) / 0.4).clamp(0.0, 1.0)
}

fn audible_motors(game: &Game) -> Vec<Motor> {
    let now = game.state.current_tick() as f32 + game.tick_fraction();
    let shells = game.state.shells();
    let mut motors = Vec::new();
    for (index, shell) in shells.iter().enumerate() {
        if shell.kind != ProjectileKind::Missile {
            continue;
        }
        let launch = world_vec(shell.launch);
        let impact = world_vec(shell.impact);
        let total = (launch.distance(impact) / oxide_sim::stats::SHELL_SPEED.to_num::<f32>())
            .ceil()
            .max(1.0);
        let elapsed = missile_elapsed_ticks(now, shell.arrival, total);
        let envelope = motor_envelope(elapsed, total);
        if envelope <= 0.0 {
            continue;
        }
        let from =
            crate::render::entities::shell_visual_origin(launch, impact, shell.shooter, shell.kind);
        let progress = crate::render::entities::missile_travel_progress(
            elapsed / total,
            total,
            from.distance(impact),
        );
        let at = from.lerp(impact, progress);
        if game.state.hostile(game.human, shell.player)
            && !game.all_seeing()
            && !game
                .my_vision()
                .visible(TilePos::new(at.x.floor() as i32, at.y.floor() as i32))
        {
            continue;
        }
        let half_extents = game.camera.viewport() / game.camera.zoom * 0.5;
        let camera_gain = crate::audio_mix::frame_mix(
            [(SoundKind::RocketMotor, Some(at))],
            game.camera.center,
            half_extents,
            game.camera.zoom,
        )[0]
        .gain;
        // Motors outside the viewport fall away fully instead of leaving the
        // one-shot mix's minimum gain as a permanent offscreen noise floor.
        let outside = ((at - game.camera.center).abs() - half_extents)
            .max(Vec2::ZERO)
            .length();
        let falloff = (1.0 - outside / half_extents.length().max(1.0)).clamp(0.0, 1.0);
        let occurrence = shells[..index]
            .iter()
            .filter(|earlier| {
                earlier.kind == shell.kind
                    && earlier.shooter == shell.shooter
                    && earlier.launch == shell.launch
                    && earlier.impact == shell.impact
                    && earlier.arrival == shell.arrival
            })
            .count();
        let gain = camera_gain * falloff * envelope;
        if gain > 0.0 {
            motors.push(Motor {
                key: RocketKey {
                    shooter: shell.shooter,
                    launch: shell.launch,
                    impact: shell.impact,
                    arrival: shell.arrival,
                    occurrence,
                },
                gain,
            });
        }
    }
    motors.sort_by(|left, right| right.gain.total_cmp(&left.gain));
    motors.truncate(MOTOR_VOICES);
    // Simultaneous launches can play the same loop in phase, so also cap
    // the coherent sum rather than assuming independent noise sources.
    let count = motors.len().max(1) as f32;
    let density = count.sqrt().recip().min(1.5 / count);
    for motor in &mut motors {
        motor.gain *= density;
    }
    motors
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    Start(usize, f32),
    Gain(usize, f32),
    Stop(usize),
}

#[derive(Default)]
pub(crate) struct RocketLoops {
    slots: [Option<RocketKey>; MOTOR_VOICES],
}

impl RocketLoops {
    fn reconcile(&mut self, desired: &[Motor]) -> Vec<Change> {
        let mut changes = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_some_and(|key| !desired.iter().any(|motor| motor.key == key)) {
                *slot = None;
                changes.push(Change::Stop(index));
            }
        }
        for motor in desired {
            if let Some(index) = self.slots.iter().position(|slot| *slot == Some(motor.key)) {
                changes.push(Change::Gain(index, motor.gain));
            } else if let Some(index) = self.slots.iter().position(Option::is_none) {
                self.slots[index] = Some(motor.key);
                changes.push(Change::Start(index, motor.gain));
            }
        }
        changes
    }

    pub(crate) fn update(&mut self, game: &Game, running: bool, clips: &[Sound], volume: f32) {
        let desired = if running && clips.len() == MOTOR_VOICES && volume > 0.0 {
            audible_motors(game)
        } else {
            Vec::new()
        };
        for change in self.reconcile(&desired) {
            match change {
                Change::Start(slot, gain) => play_sound(
                    &clips[slot],
                    PlaySoundParams {
                        looped: true,
                        volume: volume * gain,
                    },
                ),
                Change::Gain(slot, gain) => set_sound_volume(&clips[slot], volume * gain),
                Change::Stop(slot) => stop_sound(&clips[slot]),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motor(id: u32, arrival: u64) -> Motor {
        Motor {
            key: RocketKey {
                shooter: Target::Unit(oxide_sim::UnitId(id)),
                launch: Vec2Fx::ZERO,
                impact: Vec2Fx::ZERO,
                arrival,
                occurrence: 0,
            },
            gain: 0.7,
        }
    }

    #[test]
    fn overlapping_flights_stop_independently_and_do_not_retrigger_each_frame() {
        let mut loops = RocketLoops::default();
        let a = motor(1, 30);
        let b = motor(2, 40);
        assert_eq!(
            loops.reconcile(&[a, b]),
            vec![Change::Start(0, 0.7), Change::Start(1, 0.7)]
        );
        assert_eq!(
            loops.reconcile(&[b]),
            vec![Change::Stop(0), Change::Gain(1, 0.7)]
        );
        assert_eq!(loops.reconcile(&[b]), vec![Change::Gain(1, 0.7)]);
        assert_eq!(loops.reconcile(&[]), vec![Change::Stop(1)]);
        assert!(loops.reconcile(&[]).is_empty());
    }

    #[test]
    fn pause_or_seek_clears_loops_and_resuming_reconstructs_only_live_flights() {
        let mut loops = RocketLoops::default();
        loops.reconcile(&[motor(1, 30)]);
        assert_eq!(loops.reconcile(&[]), vec![Change::Stop(0)]);
        assert_eq!(
            loops.reconcile(&[motor(2, 50)]),
            vec![Change::Start(0, 0.7)]
        );
    }

    #[test]
    fn flight_envelope_is_sustained_until_arrival_including_short_flights() {
        for total in [1.0, 4.0, 20.0, 80.0] {
            let ignition = missile_ejection_ticks(total);
            assert_eq!(motor_envelope(ignition - 0.01, total), 0.0);
            assert!(motor_envelope((ignition + total) / 2.0, total) > 0.0);
            assert!(motor_envelope(total - 0.01, total) > 0.0);
            assert_eq!(motor_envelope(total, total), 0.0);
        }
    }

    #[test]
    fn real_missile_launch_flight_impact_fog_and_reconstruction_follow_state() {
        let mut map = vec![".".repeat(80); 32];
        map[20].replace_range(8..9, "1");
        map[20].replace_range(65..66, "2");
        let scenario = serde_json::from_value(serde_json::json!({
            "name": "Rocket audio lifecycle", "seed": 911, "map": map,
            "players": [
                {"name": "You", "faction": "ferrous", "bot": false},
                {"name": "Target", "faction": "cupric", "bot": true}
            ],
            "units": [
                {"player": 0, "kind": "avalanche", "x": 20, "y": 10},
                {"player": 0, "kind": "harvester", "x": 29, "y": 14}
            ],
            "buildings": [{"player": 1, "kind": "fabricator", "x": 32, "y": 10}]
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, Vec2::new(1280.0, 800.0)).unwrap();
        game.camera.center = Vec2::new(26.0, 12.0);
        for _ in 0..200 {
            game.do_tick();
            if !game.state.shells().is_empty() {
                break;
            }
        }
        assert!(
            game.sounds_pending
                .iter()
                .any(|(kind, _)| *kind == SoundKind::AvalancheFire)
        );
        assert!(
            !game
                .sounds_pending
                .iter()
                .any(|(kind, _)| *kind == SoundKind::RocketMotor)
        );
        let arrival = game.state.shells()[0].arrival;
        assert!(
            audible_motors(&game).is_empty(),
            "ejection precedes ignition"
        );
        for _ in 0..4 {
            game.do_tick();
        }
        let original = audible_motors(&game);
        assert_eq!(original.len(), 1);

        game.human = oxide_sim::PlayerId(1);
        assert!(
            audible_motors(&game).is_empty(),
            "hidden hostile motor must not reveal its muzzle"
        );
        game.overlay = true;
        assert_eq!(audible_motors(&game).len(), 1);
        game.overlay = false;
        game.human = oxide_sim::PlayerId(0);
        game.camera.center = Vec2::new(75.0, 30.0);
        assert!(
            audible_motors(&game).is_empty(),
            "distant motors must fall silent"
        );
        game.camera.center = Vec2::new(26.0, 12.0);

        let snapshot = (*game.state).clone();
        game.replace_state_after_jump(&snapshot);
        assert_eq!(audible_motors(&game)[0].key, original[0].key);
        while game.state.current_tick() <= arrival {
            game.sounds_pending.clear();
            game.do_tick();
            if game.state.current_tick() <= arrival {
                assert_eq!(audible_motors(&game).len(), 1, "motor died before arrival");
            }
        }
        assert!(audible_motors(&game).is_empty());
        assert!(
            game.sounds_pending
                .iter()
                .any(|(kind, _)| *kind == SoundKind::RocketImpact)
        );
        assert!(
            !game
                .sounds_pending
                .iter()
                .any(|(kind, _)| *kind == SoundKind::Artillery)
        );
    }
}
