use super::*;

#[test]
fn coincident_impacts_keep_projectile_order_and_player_identity() {
    use chassis::fx::Vec2Fx;
    use oxide_sim::{PlayerId, ProjectileKind, Target, UnitId};

    let shell = |kind, player| oxide_sim::state::Shell {
        kind,
        shooter: Target::Unit(UnitId(42)),
        player,
        launch: Vec2Fx::ZERO,
        impact: Vec2Fx::ZERO,
        arrival: 10,
        damage: 1,
        targets: oxide_sim::stats::DomainMask::GROUND,
        splash: None,
    };
    let mut timeline = AudioTimeline {
        arrivals: vec![
            shell(ProjectileKind::Missile, PlayerId(1)),
            shell(ProjectileKind::Shell, PlayerId(0)),
            shell(ProjectileKind::Missile, PlayerId(0)),
        ],
    };
    assert_eq!(
        timeline.landed(PlayerId(0), Vec2Fx::ZERO),
        SoundKind::Artillery
    );
    assert_eq!(
        timeline.landed(PlayerId(0), Vec2Fx::ZERO),
        SoundKind::RocketImpact
    );
    assert_eq!(
        timeline.landed(PlayerId(1), Vec2Fx::ZERO),
        SoundKind::RocketImpact
    );
    assert_eq!(
        timeline.landed(PlayerId(0), Vec2Fx::ZERO),
        SoundKind::Artillery
    );
}

#[test]
fn missile_pose_and_cues_share_the_post_tick_launch_origin() {
    assert_eq!(projectile_elapsed_ticks(11.0, 30, 20.0), 0.0);
    assert_eq!(
        projectile_elapsed_ticks(14.0, 30, 20.0),
        missile_ejection_ticks(20.0)
    );
    assert_eq!(projectile_elapsed_ticks(31.0, 30, 20.0), 20.0);
    for total in [1.0, 4.0, 20.0] {
        assert!(missile_ejection_ticks(total) < total);
    }
}
