//! Pure camera-listener weighting for one frame of queued sound events.

use crate::game::SoundKind;
use macroquad::prelude::Vec2;

const WIDE_ZOOM: f32 = 8.0;
const DETAIL_ZOOM: f32 = 32.0;
const WIDE_POSITIONAL_VOICES: usize = 5;
const CLOSE_POSITIONAL_VOICES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameSound {
    pub kind: SoundKind,
    pub gain: f32,
    positioned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeightClass {
    Detail,
    Standard,
    Heavy,
    Protected,
}

fn weight_class(kind: SoundKind) -> WeightClass {
    match kind {
        SoundKind::Alert => WeightClass::Protected,
        SoundKind::BuildingBoom
        | SoundKind::Artillery
        | SoundKind::ArtilleryLaunch
        | SoundKind::BombardFire
        | SoundKind::BastionFire => WeightClass::Heavy,
        SoundKind::UnitDeath
        | SoundKind::LancerFire
        | SoundKind::FlakhoundFire
        | SoundKind::BuzzardFire
        | SoundKind::FlakTurretFire => WeightClass::Standard,
        SoundKind::Laser
        | SoundKind::ScuttlerFire
        | SoundKind::SentinelFire
        | SoundKind::StingerFire
        | SoundKind::DarterFire
        | SoundKind::TalonFire
        | SoundKind::WispFire => WeightClass::Detail,
        SoundKind::Deposit
        | SoundKind::TrainDone
        | SoundKind::Click
        | SoundKind::Denied
        | SoundKind::Victory
        | SoundKind::Defeat
        | SoundKind::Ack => WeightClass::Standard,
    }
}

fn zoom_detail(zoom: f32) -> f32 {
    ((zoom - WIDE_ZOOM) / (DETAIL_ZOOM - WIDE_ZOOM)).clamp(0.0, 1.0)
}

fn zoom_gain(kind: SoundKind, zoom: f32) -> f32 {
    let detail = zoom_detail(zoom);
    match weight_class(kind) {
        WeightClass::Detail => 0.25 + 0.75 * detail,
        WeightClass::Standard => 0.45 + 0.55 * detail,
        WeightClass::Heavy => 0.72 + 0.28 * detail,
        WeightClass::Protected => 1.0,
    }
}

fn distance_gain(world: Vec2, center: Vec2, half_extents: Vec2) -> f32 {
    let half_extents = Vec2::new(half_extents.x.max(1.0), half_extents.y.max(1.0));
    let delta = (world - center).abs() - half_extents;
    let outside = Vec2::new(delta.x.max(0.0), delta.y.max(0.0));
    let distance = outside.length();
    if distance == 0.0 {
        1.0
    } else {
        (1.0 - distance / (2.0 * half_extents.length())).clamp(0.25, 1.0)
    }
}

fn positional_voice_limit(zoom: f32) -> usize {
    let detail = zoom_detail(zoom);
    WIDE_POSITIONAL_VOICES
        + ((CLOSE_POSITIONAL_VOICES - WIDE_POSITIONAL_VOICES) as f32 * detail).round() as usize
}

/// Coalesces one frame of emitters into the mix heard at the current camera.
///
/// Unpositioned UI and alert sounds always survive. Positional duplicates keep
/// the loudest emitter of their kind, and a wide camera admits fewer minor
/// voices while reserving room for heavy threats.
pub(crate) fn frame_mix(
    queued: impl IntoIterator<Item = (SoundKind, Option<Vec2>)>,
    center: Vec2,
    half_extents: Vec2,
    zoom: f32,
) -> Vec<FrameSound> {
    let mut mixed: Vec<FrameSound> = Vec::new();
    for (kind, world) in queued {
        let protected = matches!(kind, SoundKind::Alert);
        let positioned = world.is_some() && !protected;
        let gain = if protected {
            1.0
        } else if let Some(world) = world {
            distance_gain(world, center, half_extents) * zoom_gain(kind, zoom)
        } else {
            1.0
        };

        if let Some(existing) = mixed.iter_mut().find(|event| event.kind == kind) {
            if gain > existing.gain {
                existing.gain = gain;
                existing.positioned = positioned;
            }
        } else {
            mixed.push(FrameSound {
                kind,
                gain,
                positioned,
            });
        }
    }

    let limit = positional_voice_limit(zoom);
    let mut ranked: Vec<usize> = mixed
        .iter()
        .enumerate()
        .filter_map(|(index, event)| event.positioned.then_some(index))
        .collect();
    ranked.sort_by(|&left, &right| {
        let heavy = |event: FrameSound| matches!(weight_class(event.kind), WeightClass::Heavy);
        heavy(mixed[right])
            .cmp(&heavy(mixed[left]))
            .then_with(|| mixed[right].gain.total_cmp(&mixed[left].gain))
            .then_with(|| left.cmp(&right))
    });
    if ranked.len() > limit {
        let mut keep = vec![true; mixed.len()];
        for index in ranked.into_iter().skip(limit) {
            keep[index] = false;
        }
        let mut index = 0;
        mixed.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
    }

    mixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    #[test]
    fn close_camera_exposes_more_minor_detail_than_wide_camera() {
        let event = [(SoundKind::ScuttlerFire, Some(vec2(0.0, 0.0)))];
        let wide = frame_mix(event, Vec2::ZERO, vec2(40.0, 25.0), WIDE_ZOOM);
        let close = frame_mix(event, Vec2::ZERO, vec2(10.0, 6.0), DETAIL_ZOOM);

        assert!(close[0].gain > wide[0].gain * 3.0);
    }

    #[test]
    fn heavy_reports_remain_more_legible_at_wide_zoom() {
        let mixed = frame_mix(
            [
                (SoundKind::Laser, Some(Vec2::ZERO)),
                (SoundKind::BastionFire, Some(Vec2::ZERO)),
            ],
            Vec2::ZERO,
            vec2(40.0, 25.0),
            WIDE_ZOOM,
        );
        let gain = |kind| mixed.iter().find(|event| event.kind == kind).unwrap().gain;

        assert!(gain(SoundKind::BastionFire) > gain(SoundKind::Laser) * 2.0);
    }

    #[test]
    fn massed_equal_reports_coalesce_to_the_loudest_emitter() {
        let mixed = frame_mix(
            [
                (SoundKind::Laser, Some(vec2(160.0, 0.0))),
                (SoundKind::Laser, Some(vec2(8.0, 0.0))),
                (SoundKind::Laser, Some(Vec2::ZERO)),
            ],
            Vec2::ZERO,
            vec2(20.0, 12.0),
            DETAIL_ZOOM,
        );

        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].gain, 1.0);
    }

    #[test]
    fn wide_mix_bounds_distinct_minor_voices_but_keeps_heavy_threats() {
        let mixed = frame_mix(
            [
                (SoundKind::Laser, Some(Vec2::ZERO)),
                (SoundKind::ScuttlerFire, Some(Vec2::ZERO)),
                (SoundKind::SentinelFire, Some(Vec2::ZERO)),
                (SoundKind::StingerFire, Some(Vec2::ZERO)),
                (SoundKind::DarterFire, Some(Vec2::ZERO)),
                (SoundKind::TalonFire, Some(Vec2::ZERO)),
                (SoundKind::WispFire, Some(Vec2::ZERO)),
                (SoundKind::UnitDeath, Some(Vec2::ZERO)),
                (SoundKind::BastionFire, Some(Vec2::ZERO)),
            ],
            Vec2::ZERO,
            vec2(40.0, 25.0),
            WIDE_ZOOM,
        );

        assert_eq!(mixed.iter().filter(|event| event.positioned).count(), 5);
        assert!(
            mixed
                .iter()
                .any(|event| event.kind == SoundKind::BastionFire)
        );
    }

    #[test]
    fn attack_alert_bypasses_position_zoom_and_voice_budget() {
        let mut queued = vec![(SoundKind::Alert, Some(vec2(10_000.0, 10_000.0)))];
        queued.extend([
            (SoundKind::Laser, Some(Vec2::ZERO)),
            (SoundKind::ScuttlerFire, Some(Vec2::ZERO)),
            (SoundKind::SentinelFire, Some(Vec2::ZERO)),
            (SoundKind::StingerFire, Some(Vec2::ZERO)),
            (SoundKind::DarterFire, Some(Vec2::ZERO)),
            (SoundKind::TalonFire, Some(Vec2::ZERO)),
        ]);
        let mixed = frame_mix(queued, Vec2::ZERO, vec2(40.0, 25.0), WIDE_ZOOM);
        let alert = mixed
            .iter()
            .find(|event| event.kind == SoundKind::Alert)
            .unwrap();

        assert_eq!(alert.gain, 1.0);
        assert!(!alert.positioned);
    }

    #[test]
    fn unpositioned_ui_survives_the_positional_voice_budget() {
        let mut queued = vec![(SoundKind::Click, None)];
        queued.extend([
            (SoundKind::Laser, Some(Vec2::ZERO)),
            (SoundKind::ScuttlerFire, Some(Vec2::ZERO)),
            (SoundKind::SentinelFire, Some(Vec2::ZERO)),
            (SoundKind::StingerFire, Some(Vec2::ZERO)),
            (SoundKind::DarterFire, Some(Vec2::ZERO)),
            (SoundKind::TalonFire, Some(Vec2::ZERO)),
        ]);
        let mixed = frame_mix(queued, Vec2::ZERO, vec2(40.0, 25.0), WIDE_ZOOM);
        let click = mixed
            .iter()
            .find(|event| event.kind == SoundKind::Click)
            .unwrap();

        assert_eq!(click.gain, 1.0);
        assert!(!click.positioned);
    }

    #[test]
    fn portrait_view_uses_both_camera_extents() {
        assert_eq!(
            distance_gain(vec2(0.0, 30.0), Vec2::ZERO, vec2(10.0, 40.0)),
            1.0
        );
        assert!(distance_gain(vec2(30.0, 0.0), Vec2::ZERO, vec2(10.0, 40.0)) < 1.0);
    }

    #[test]
    fn nearby_detail_outweighs_a_far_standard_report() {
        let mixed = frame_mix(
            [
                (SoundKind::UnitDeath, Some(vec2(10_000.0, 0.0))),
                (SoundKind::Laser, Some(Vec2::ZERO)),
                (SoundKind::ScuttlerFire, Some(Vec2::ZERO)),
                (SoundKind::SentinelFire, Some(Vec2::ZERO)),
                (SoundKind::StingerFire, Some(Vec2::ZERO)),
                (SoundKind::DarterFire, Some(Vec2::ZERO)),
            ],
            Vec2::ZERO,
            vec2(40.0, 25.0),
            WIDE_ZOOM,
        );

        assert!(!mixed.iter().any(|event| event.kind == SoundKind::UnitDeath));
        assert!(mixed.iter().any(|event| event.kind == SoundKind::Laser));
    }
}
