//! Motor loops driven by the currently presented missile state.

use crate::numeric;
use chassis::fx::Vec2Fx;
use macroquad::audio::{PlaySoundParams, Sound, play_sound, set_sound_volume, stop_sound};
use macroquad::prelude::Vec2;
use oxide_sim::{ProjectileKind, Target};

use crate::audio_timeline::{missile_ejection_ticks, projectile_elapsed_ticks};
use crate::game::{Scene, SoundKind, world_vec};

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

fn audible_motors(game: &Scene<'_>) -> Vec<Motor> {
    let now = game.state.current_tick() as f32 + game.presentation.tick_fraction();
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
        let elapsed = projectile_elapsed_ticks(now, shell.arrival, total);
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
        if game.state.hostile(game.presentation.human, shell.player)
            && !game.presentation.all_seeing()
            && !game.my_vision().visible(numeric::tile_at(at))
        {
            continue;
        }
        let half_extents =
            game.presentation.camera.viewport() / game.presentation.camera.zoom * 0.5;
        let camera_gain = crate::audio_mix::frame_mix(
            [(SoundKind::RocketMotor, Some(at))],
            game.presentation.camera.center,
            half_extents,
            game.presentation.camera.zoom,
        )[0]
        .gain;
        // Motors outside the viewport fall away fully instead of leaving the
        // one-shot mix's minimum gain as a permanent offscreen noise floor.
        let outside = ((at - game.presentation.camera.center).abs() - half_extents)
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

    pub(crate) fn update(&mut self, game: &Scene<'_>, running: bool, clips: &[Sound], volume: f32) {
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
mod tests;
