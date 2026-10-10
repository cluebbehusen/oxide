//! The sound mixer and the one table of what the shell decides per sound.

use crate::assets;
use crate::config;
use crate::game::SoundKind;
use crate::numeric;
use macroquad::audio::{PlaySoundParams, Sound, play_sound};
use macroquad::prelude::get_time;

/// Which settings bus a clip bills against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bus {
    /// Chrome sounds.
    Ui,
    /// Everything the battlefield makes.
    Effects,
}

/// How a clip competes for one frame's positional voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Weight {
    /// Small arms that thin out first when the camera pulls back.
    Detail,
    /// Ordinary reports and cues.
    Standard,
    /// Blasts and heavy guns that keep a voice at any zoom.
    Heavy,
    /// Never dropped from the mix.
    Protected,
}

/// Everything the shell decides about one sound kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SoundSpec {
    /// The generated clip, which also names its row in the sound manifest.
    /// The basic zap alternates with `laser2`, a clip the manifest gives the
    /// same mixing.
    pub clip: &'static str,
    pub bus: Bus,
    pub weight: Weight,
    /// Whether hearing it raises the score's combat bed.
    pub combat: bool,
    /// Whether it is a blast, heard from beyond sight with its own falloff.
    pub explosion: bool,
}

const fn row(
    clip: &'static str,
    bus: Bus,
    weight: Weight,
    combat: bool,
    explosion: bool,
) -> SoundSpec {
    SoundSpec {
        clip,
        bus,
        weight,
        combat,
        explosion,
    }
}

/// The per-sound table: a new kind adds exactly one row here.
pub(crate) const fn spec(kind: SoundKind) -> SoundSpec {
    use Bus::{Effects, Ui};
    use Weight::{Detail, Heavy, Protected, Standard};
    match kind {
        SoundKind::Laser => row("laser", Effects, Detail, true, false),
        SoundKind::UnitDeath => row("unit_death", Effects, Standard, true, false),
        SoundKind::BuildingBoom => row("building_boom", Effects, Heavy, true, true),
        SoundKind::Deposit => row("deposit", Effects, Standard, false, false),
        SoundKind::TrainDone => row("train_done", Effects, Standard, false, false),
        SoundKind::Click => row("click", Ui, Standard, false, false),
        SoundKind::Denied => row("denied", Ui, Standard, false, false),
        SoundKind::Alert => row("alert", Effects, Protected, true, false),
        SoundKind::Victory => row("victory", Effects, Standard, false, false),
        SoundKind::Defeat => row("defeat", Effects, Standard, false, false),
        SoundKind::Artillery => row("artillery_boom", Effects, Heavy, true, true),
        SoundKind::ArtilleryLaunch => row("artillery_launch", Effects, Heavy, true, false),
        SoundKind::Ack => row("ack", Effects, Standard, false, false),
        SoundKind::SentinelFire => row("attack_sentinel", Effects, Detail, true, false),
        SoundKind::ScuttlerFire => row("attack_scuttler", Effects, Detail, true, false),
        SoundKind::LancerFire => row("attack_lancer", Effects, Standard, true, false),
        SoundKind::BombardFire => row("attack_bombard", Effects, Heavy, true, false),
        SoundKind::FlakhoundFire => row("attack_flakhound", Effects, Standard, true, false),
        SoundKind::StingerFire => row("attack_stinger", Effects, Detail, true, false),
        SoundKind::BuzzardFire => row("attack_buzzard", Effects, Standard, true, false),
        SoundKind::DarterFire => row("attack_darter", Effects, Detail, true, false),
        SoundKind::TalonFire => row("attack_talon", Effects, Detail, true, false),
        SoundKind::WispFire => row("attack_wisp", Effects, Detail, true, false),
        SoundKind::BastionFire => row("attack_bastion", Effects, Heavy, true, false),
        SoundKind::FlakTurretFire => row("attack_flak_turret", Effects, Standard, true, false),
        SoundKind::WardenFire => row("attack_warden", Effects, Detail, true, false),
        SoundKind::BreakerFire => row("attack_breaker", Effects, Heavy, true, false),
        SoundKind::AvalancheFire => row("avalanche_launch", Effects, Heavy, true, false),
        SoundKind::RocketMotor => row("avalanche_motor", Effects, Heavy, true, false),
        SoundKind::RocketImpact => row("rocket_impact", Effects, Heavy, true, true),
        SoundKind::BombRelease => row("bomb_release", Effects, Heavy, true, false),
        SoundKind::DemolitionBoom => row("demolition_boom", Effects, Heavy, true, true),
        SoundKind::UpgradeDone => row("upgrade_done", Effects, Standard, false, false),
    }
}

/// One clip's mixing, as `tools/gen_sounds.py` records it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MixerSpec {
    volume: f32,
    min_gap: f64,
}

/// The generated sound manifest is the one source for mixer gain and rate
/// limits.
static MIXER_SPECS: std::sync::LazyLock<std::collections::BTreeMap<String, MixerSpec>> =
    std::sync::LazyLock::new(|| {
        #[derive(serde::Deserialize)]
        struct Manifest {
            sounds: Vec<Row>,
        }
        #[derive(serde::Deserialize)]
        struct Row {
            name: String,
            mixer_volume: f64,
            min_gap: f64,
        }
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../assets/sounds/manifest.json"))
                .expect("the generated sound manifest parses");
        manifest
            .sounds
            .into_iter()
            .map(|row| {
                let spec = MixerSpec {
                    volume: numeric::to_f32(row.mixer_volume),
                    min_gap: row.min_gap,
                };
                (row.name, spec)
            })
            .collect()
    });

fn mixer_spec(kind: SoundKind) -> MixerSpec {
    MIXER_SPECS
        .get(spec(kind).clip)
        .copied()
        .expect("every sound kind has a manifest row")
}

/// Plays queued clips with a per-kind rate limit, so twenty simultaneous
/// weapon reports read as battle, not noise.
#[derive(Default)]
pub(crate) struct Mixer {
    pub(crate) rocket_loops: crate::rocket_audio::RocketLoops,
    last_played: std::collections::HashMap<SoundKind, f64>,
    /// Alternates the basic zap between two clips so volleys read as
    /// many guns, not one sample looping.
    laser_flip: bool,
}

impl Mixer {
    /// The settings gain a clip bills against.
    fn bus(volumes: &config::Volumes, kind: SoundKind) -> f32 {
        let bus = match spec(kind).bus {
            Bus::Ui => volumes.ui,
            Bus::Effects => volumes.effects,
        };
        volumes.master * bus
    }

    /// Seconds a clip waits before it may play again.
    fn min_gap(kind: SoundKind) -> f64 {
        mixer_spec(kind).min_gap
    }

    pub(crate) fn base_volume(kind: SoundKind) -> f32 {
        mixer_spec(kind).volume
    }

    fn clip<'a>(&mut self, sounds: &'a assets::Sounds, kind: SoundKind) -> &'a Sound {
        if kind == SoundKind::Laser {
            self.laser_flip = !self.laser_flip;
            if !self.laser_flip {
                return &sounds.laser2;
            }
        }
        sounds.effect(kind)
    }

    pub(crate) fn play(
        &mut self,
        sounds: &assets::Sounds,
        kind: SoundKind,
        volumes: &config::Volumes,
        attenuation: f32,
    ) {
        let now = get_time();
        let min_gap = Self::min_gap(kind);
        if now - self.last_played.get(&kind).copied().unwrap_or(f64::MIN) < min_gap {
            return;
        }
        self.last_played.insert(kind, now);
        let volume = Self::base_volume(kind) * Self::bus(volumes, kind) * attenuation;
        if volume <= 0.0 {
            return;
        }
        let sound = self.clip(sounds, kind);
        play_sound(
            sound,
            PlaySoundParams {
                looped: false,
                volume,
            },
        );
    }
}

#[cfg(test)]
mod tests;
