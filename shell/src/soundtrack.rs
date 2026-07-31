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
mod tests {
    use super::*;

    fn settle(score: &mut Soundtrack, scene: Scene, volumes: Volumes) -> Mix {
        let mut mix = Mix::default();
        for _ in 0..100 {
            mix = score.update(scene, false, 0.1, volumes);
        }
        mix
    }

    #[test]
    fn menu_and_match_crossfade_without_a_restart_or_jump() {
        let volumes = Volumes::default();
        let mut score = Soundtrack::default();
        let menu = settle(&mut score, Scene::Menu, volumes);
        assert_eq!(menu.menu, 0.16);
        assert_eq!(menu.calm, 0.0);

        let first_match = score.update(Scene::Match, false, 0.1, volumes);
        assert!(
            first_match.menu > 0.0 && first_match.menu < menu.menu,
            "the old bed fades instead of stopping"
        );
        assert!(
            first_match.calm > 0.0 && first_match.calm < 0.16,
            "the new bed fades in instead of restarting loudly"
        );
        let calm = settle(&mut score, Scene::Match, volumes);
        assert_eq!(calm.menu, 0.0);
        assert_eq!(calm.calm, 0.16);
    }

    #[test]
    fn a_combat_impulse_layers_pressure_then_returns_to_calm() {
        let volumes = Volumes::default();
        let mut score = Soundtrack::default();
        settle(&mut score, Scene::Match, volumes);
        score.update(Scene::Match, true, 0.1, volumes);
        let mut battle = Mix::default();
        for _ in 0..10 {
            battle = score.update(Scene::Match, false, 0.1, volumes);
        }
        assert!(battle.combat > 0.1, "one volley has a useful musical tail");
        assert!(
            battle.calm < 0.16,
            "the pressure layer makes room for itself"
        );

        let calm = settle(&mut score, Scene::Match, volumes);
        assert_eq!(calm.combat, 0.0);
        assert_eq!(calm.calm, 0.16);
    }

    #[test]
    fn pause_ducks_the_match_bed_without_silencing_it() {
        let volumes = Volumes::default();
        let mut score = Soundtrack::default();
        settle(&mut score, Scene::Match, volumes);
        let paused = settle(&mut score, Scene::Pause, volumes);
        assert_eq!(paused.calm, 0.05);
        assert_eq!(paused.combat, 0.0);
    }

    #[test]
    fn neutral_victory_and_defeat_results_have_distinct_beds() {
        for (scene, field) in [(Scene::Result, 3), (Scene::Victory, 4), (Scene::Defeat, 5)] {
            let mut score = Soundtrack::default();
            let mix = settle(&mut score, scene, Volumes::default());
            let values = [
                mix.menu,
                mix.calm,
                mix.combat,
                mix.result,
                mix.victory,
                mix.defeat,
            ];
            assert!(values[field] > 0.14);
            assert!(
                values
                    .iter()
                    .enumerate()
                    .all(|(index, value)| index == field || *value == 0.0),
                "{scene:?} owns exactly one result bed"
            );
        }
    }

    #[test]
    fn mute_and_volume_edits_fade_and_scale_the_music_bus() {
        let mut score = Soundtrack::default();
        let full = settle(&mut score, Scene::Menu, Volumes::default());
        let muted = Volumes {
            music: 0.0,
            ..Volumes::default()
        };
        let first_muted = score.update(Scene::Menu, false, 0.1, muted);
        assert!(
            first_muted.menu > 0.0 && first_muted.menu < full.menu,
            "mute is a short fade, not a click"
        );
        assert_eq!(settle(&mut score, Scene::Menu, muted), Mix::default());

        let half = Volumes {
            master: 0.5,
            music: 0.5,
            ..Volumes::default()
        };
        let mix = settle(&mut score, Scene::Menu, half);
        assert!((mix.menu - 0.04).abs() < 1.0e-6);
    }
}
