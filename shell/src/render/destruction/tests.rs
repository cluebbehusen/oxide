use super::*;
use crate::game::Game;

#[test]
fn scheduled_airframe_contacts_the_damage_point_with_its_full_level_pose() {
    for kind in [oxide_sim::UnitKind::Condor, oxide_sim::UnitKind::Skyhook] {
        let crash = oxide_sim::state::AircraftCrash {
            unit: oxide_sim::UnitId(0),
            player: oxide_sim::PlayerId(0),
            kind,
            heading: 0,
            launch: chassis::grid::TilePos::new(8, 8).center(),
            impact: chassis::grid::TilePos::new(9, 8).center(),
            started: 10,
            arrival: 23,
        };
        let body = UnitBody {
            kind,
            player: crash.player,
            rotation: 1.2,
            velocity: Vec2::ZERO,
        };
        let start = fall_pose(Vec2::ZERO, body, 0.0, Some(crash));
        let halfway = fall_pose(Vec2::ZERO, body, CRASH_TIME / 2.0, Some(crash));
        let contact = fall_pose(Vec2::ZERO, body, CRASH_TIME, Some(crash));
        assert!(start.body_at.x < halfway.body_at.x && halfway.body_at.x < contact.body_at.x);
        assert_eq!(contact.at, vec2(9.5, 8.5));
        assert_eq!(contact.body_at, contact.at);
        assert_eq!(start.rotation, contact.rotation);
    }
}

#[test]
fn own_crash_remains_visible_when_the_casualty_was_the_last_vision_source() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.map = vec![".".repeat(40); 26];
    scenario.map[3].replace_range(2..3, "1");
    scenario.map[3].replace_range(35..36, "2");
    for player in &mut scenario.players {
        player.bot_config = None;
    }
    scenario.units = vec![oxide_sim::scenario::UnitSpec {
        player: 0,
        kind: oxide_sim::UnitKind::Condor,
        x: 25,
        y: 20,
    }];
    for y in [17, 20, 23] {
        scenario.units.push(oxide_sim::scenario::UnitSpec {
            player: 1,
            kind: oxide_sim::UnitKind::Flakhound,
            x: 28,
            y,
        });
    }
    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
    for _ in 0..600 {
        game.present_ticks(1);
        if let Some((at, body)) = game
            .presentation
            .fx
            .iter()
            .find_map(|effect| match effect.kind {
                EffectKind::Falling { at, body, .. } => Some((at, body)),
                _ => None,
            })
        {
            assert!(!visible(&game.view(), at));
            assert!(casualty_visible(&game.view(), at, body));
            assert!(!casualty_visible(
                &game.view(),
                at,
                UnitBody {
                    player: oxide_sim::PlayerId(1),
                    ..body
                },
            ));
            return;
        }
    }
    panic!("the flak line did not destroy the aircraft");
}

#[test]
fn fragments_land_once_and_stop_without_bouncing() {
    let at_contact = fragment_pose(0.3, 0.3, 0.7, 0.2);
    let after_skid = fragment_pose(0.55, 0.3, 0.7, 0.2);
    assert_eq!(at_contact.1, 0.0);
    assert_eq!(after_skid.1, 0.0);
    assert_eq!(after_skid, fragment_pose(8.0, 0.3, 0.7, 0.2));
    assert!(after_skid.0 > at_contact.0);
}

#[test]
fn aircraft_contact_preserves_heading_and_stops_the_coast() {
    let body = UnitBody {
        kind: oxide_sim::UnitKind::Condor,
        player: oxide_sim::PlayerId(0),
        rotation: 2.1,
        velocity: vec2(-2.0, 1.0),
    };
    let start = crash_pose(vec2(8.0, 8.0), body, 0.0, false);
    let contact = crash_pose(vec2(8.0, 8.0), body, CRASH_TIME, false);
    let settled = crash_pose(vec2(8.0, 8.0), body, 3.0, false);
    assert_eq!(start.rotation, body.rotation);
    assert!(start.body_at.y < start.at.y);
    assert_eq!(contact.body_at, contact.at);
    assert_eq!(contact.at, settled.at);
    assert_eq!(contact.rotation, body.rotation);
    assert!(contact.at.x < start.at.x && contact.at.y > start.at.y);
    assert_eq!(crash_pose(start.at, body, 0.0, true).at, start.at);
}

#[test]
fn a_hovering_airframe_drops_level_to_its_existing_shadow() {
    let body = UnitBody {
        kind: oxide_sim::UnitKind::Skyhook,
        player: oxide_sim::PlayerId(0),
        rotation: 0.0,
        velocity: Vec2::ZERO,
    };
    let start = crash_pose(Vec2::ZERO, body, 0.0, false);
    let middle = crash_pose(Vec2::ZERO, body, CRASH_TIME * 0.5, false);
    let contact = crash_pose(Vec2::ZERO, body, CRASH_TIME, false);
    let (_, offset, lift) = air_presentation(body.kind, 1.0);
    assert_eq!(start.body_at, vec2(0.0, -lift));
    assert_eq!(start.at, offset);
    assert_eq!(contact.at, start.at);
    assert_eq!(contact.body_at, contact.at);
    assert!(middle.body_at.y > start.body_at.y && middle.body_at.y < contact.body_at.y);
    assert_eq!(middle.rotation, start.rotation);
    for kind in [oxide_sim::UnitKind::Condor, oxide_sim::UnitKind::Skyhook] {
        assert!(large_airframe(kind));
    }
    for kind in [
        oxide_sim::UnitKind::Kestrel,
        oxide_sim::UnitKind::Buzzard,
        oxide_sim::UnitKind::Talon,
    ] {
        assert!(!large_airframe(kind));
    }
}

#[test]
fn small_airframe_breaks_above_ground_then_its_fragments_settle() {
    let body = UnitBody {
        kind: oxide_sim::UnitKind::Kestrel,
        player: oxide_sim::PlayerId(0),
        rotation: 0.0,
        velocity: vec2(1.0, 0.0),
    };
    let left = air_fragment(body, 7, 0, 0.0);
    let right = air_fragment(body, 7, 1, 0.0);
    assert!(left.lift > 0.0 && right.lift > 0.0);
    assert!(left.offset.x < right.offset.x);
    for i in 0..4 {
        let landed = air_fragment(body, 7, i, 1.0);
        let expired = air_fragment(body, 7, i, 2.0);
        assert_eq!(landed.lift, 0.0);
        assert_eq!(landed.offset, expired.offset);
        assert_eq!(landed.rotation, expired.rotation);
        assert_eq!(expired.alpha, 0.0);
    }
}

#[test]
fn structural_fragments_stop_rotating_and_leave_gaps() {
    let retained = collapse_piece(1, 0.8, 7);
    let late = collapse_piece(1, 3.0, 7);
    assert_eq!(retained.offset, late.offset);
    assert_eq!(retained.rotation, late.rotation);
    assert!(retained.rotation.abs() < 0.4);
    assert!(retained.alpha > 0.0);
    assert_eq!(collapse_piece(0, 0.8, 7).alpha, 0.0);
}
