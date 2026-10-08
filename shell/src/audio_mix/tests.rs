use super::*;
use macroquad::prelude::vec2;

#[test]
fn explosions_fade_to_silence_beyond_each_camera_edge_at_every_zoom() {
    for kind in [
        SoundKind::Artillery,
        SoundKind::RocketImpact,
        SoundKind::DemolitionBoom,
        SoundKind::BuildingBoom,
    ] {
        for (zoom, extents) in [
            (WIDE_ZOOM, vec2(40.0, 25.0)),
            (DETAIL_ZOOM, vec2(10.0, 6.0)),
            (DETAIL_ZOOM, vec2(6.0, 10.0)),
        ] {
            let center = vec2(80.0, 50.0);
            for axis in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                let edge = center + axis * extents;
                for (distance, expected) in [(0.0, 1.0), (12.0, 0.5), (24.0, 0.0), (200.0, 0.0)] {
                    let mixed = frame_mix(
                        [(kind, Some(edge + axis * distance))],
                        center,
                        extents,
                        zoom,
                    );
                    if expected == 0.0 {
                        assert!(mixed.is_empty(), "{kind:?} at {distance}");
                    } else {
                        assert_eq!(mixed.len(), 1);
                        assert!((mixed[0].gain - expected * zoom_gain(kind, zoom)).abs() < 1e-6);
                    }
                }
            }
            let diagonal = extents + vec2(18.0, 18.0);
            assert!(frame_mix([(kind, Some(center + diagonal))], center, extents, zoom).is_empty());
        }
    }
}

#[test]
fn overlapping_explosions_keep_nearest_and_distant_blasts_consume_no_voices() {
    let mut queued = vec![(SoundKind::RocketImpact, Some(vec2(22.0, 0.0))); 100];
    queued.push((SoundKind::RocketImpact, Some(Vec2::ZERO)));
    for kind in [
        SoundKind::Artillery,
        SoundKind::DemolitionBoom,
        SoundKind::BuildingBoom,
    ] {
        queued.push((kind, Some(vec2(1000.0, 0.0))));
    }
    for kind in [
        SoundKind::Laser,
        SoundKind::ScuttlerFire,
        SoundKind::SentinelFire,
        SoundKind::StingerFire,
    ] {
        queued.push((kind, Some(Vec2::ZERO)));
    }
    queued.push((SoundKind::Alert, None));
    let mixed = frame_mix(queued, Vec2::ZERO, vec2(10.0, 6.0), WIDE_ZOOM);
    assert_eq!(mixed.len(), 6);
    assert_eq!(
        mixed
            .iter()
            .filter(|sound| sound.kind == SoundKind::RocketImpact)
            .count(),
        1
    );
    assert_eq!(
        mixed
            .iter()
            .find(|sound| sound.kind == SoundKind::RocketImpact)
            .unwrap()
            .gain,
        0.72
    );
    assert_eq!(
        mixed
            .iter()
            .find(|sound| sound.kind == SoundKind::Alert)
            .unwrap()
            .gain,
        1.0
    );
    assert!(
        !mixed
            .iter()
            .any(|sound| sound.kind == SoundKind::BuildingBoom)
    );
}

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
        distance_gain(
            SoundKind::Laser,
            vec2(0.0, 30.0),
            Vec2::ZERO,
            vec2(10.0, 40.0)
        ),
        1.0
    );
    assert!(
        distance_gain(
            SoundKind::Laser,
            vec2(30.0, 0.0),
            Vec2::ZERO,
            vec2(10.0, 40.0)
        ) < 1.0
    );
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
