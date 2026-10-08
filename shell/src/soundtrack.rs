//! Presentation-only score mixing.
//!
//! Every generated bed starts once and keeps its playhead. Screen changes,
//! combat, pausing, and volume edits only crossfade those beds, so returning
//! to a match never restarts a loop at its first beat.

use crate::assets::Sounds;
use crate::config::Volumes;
use macroquad::audio::{PlaySoundParams, play_sound, set_sound_volume};

/// The presentation context the score reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scene {
    /// Home, setup, replay shelf, or Settings opened from Home.
    Menu,
    /// A live or replayed match with no nearby combat.
    Match,
    /// An ongoing match behind Pause or Settings.
    Pause,
    /// A drawn match.
    Result,
    /// The local player won.
    Victory,
    /// The local player lost or surrendered.
    Defeat,
}

/// Authored gains for every continuously running bed.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Mix {
    pub menu: f32,
    pub calm: f32,
    pub combat: f32,
    pub result: f32,
    pub victory: f32,
    pub defeat: f32,
}

impl Mix {
    fn scaled(self, bus: f32) -> Self {
        Self {
            menu: self.menu * bus,
            calm: self.calm * bus,
            combat: self.combat * bus,
            result: self.result * bus,
            victory: self.victory * bus,
            defeat: self.defeat * bus,
        }
    }

    fn approach(&mut self, target: Self, delta: f32) {
        fn one(value: &mut f32, target: f32, delta: f32) {
            if *value < target {
                *value = (*value + delta).min(target);
            } else {
                *value = (*value - delta).max(target);
            }
        }
        one(&mut self.menu, target.menu, delta);
        one(&mut self.calm, target.calm, delta);
        one(&mut self.combat, target.combat, delta);
        one(&mut self.result, target.result, delta);
        one(&mut self.victory, target.victory, delta);
        one(&mut self.defeat, target.defeat, delta);
    }
}

/// Pure mix state plus the thin macroquad playback adapter.
pub(crate) struct Soundtrack {
    mix: Mix,
    combat_energy: f32,
    started: bool,
}

impl Default for Soundtrack {
    fn default() -> Self {
        Self {
            mix: Mix::default(),
            combat_energy: 0.0,
            started: false,
        }
    }
}

impl Soundtrack {
    /// Starts every bed silently and exactly once.
    pub fn start(&mut self, sounds: &Sounds) {
        if self.started {
            return;
        }
        for sound in [
            &sounds.music_menu,
            &sounds.music_calm,
            &sounds.music_combat,
            &sounds.music_result,
            &sounds.music_victory,
            &sounds.music_defeat,
        ] {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: true,
                    volume: 0.0,
                },
            );
        }
        self.started = true;
    }

    /// Advances the pure crossfade and returns the resulting authored gains.
    pub fn update(&mut self, scene: Scene, combat_impulse: bool, dt: f32, volumes: Volumes) -> Mix {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.25)
        } else {
            0.0
        };
        if scene == Scene::Match && combat_impulse {
            self.combat_energy = 1.0;
        } else {
            let release = if scene == Scene::Match { 0.13 } else { 0.5 };
            self.combat_energy = (self.combat_energy - release * dt).max(0.0);
        }

        let target = match scene {
            Scene::Menu => Mix {
                menu: 0.16,
                ..Mix::default()
            },
            Scene::Match => Mix {
                calm: 0.16 - 0.035 * self.combat_energy,
                combat: 0.15 * self.combat_energy,
                ..Mix::default()
            },
            Scene::Pause => Mix {
                calm: 0.05,
                ..Mix::default()
            },
            Scene::Result => Mix {
                result: 0.15,
                ..Mix::default()
            },
            Scene::Victory => Mix {
                victory: 0.17,
                ..Mix::default()
            },
            Scene::Defeat => Mix {
                defeat: 0.15,
                ..Mix::default()
            },
        };
        let sane = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let target = target.scaled(sane(volumes.master) * sane(volumes.music));
        self.mix.approach(target, 0.28 * dt);
        self.mix
    }

    /// Applies the current pure mix to the already-running beds.
    pub fn apply(&mut self, sounds: &Sounds) {
        self.start(sounds);
        set_sound_volume(&sounds.music_menu, self.mix.menu);
        set_sound_volume(&sounds.music_calm, self.mix.calm);
        set_sound_volume(&sounds.music_combat, self.mix.combat);
        set_sound_volume(&sounds.music_result, self.mix.result);
        set_sound_volume(&sounds.music_victory, self.mix.victory);
        set_sound_volume(&sounds.music_defeat, self.mix.defeat);
    }
}

#[cfg(test)]
mod tests;
