//! Scuttler melee contact and unchanged bite cadence.
mod common;
use chassis::fx::Fx;
use common::{cmd, open_arena, unit};
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Command, Event, Target, UnitKind};

#[test]
fn scuttler_closes_to_building_contact_from_faces_and_corners() {
    for start in [(7, 12), (18, 12), (12, 7), (12, 18), (7, 7), (18, 18)] {
        let mut scenario = open_arena(
            30,
            24,
            vec![
                unit(0, UnitKind::Scuttler, start.0, start.1),
                unit(0, UnitKind::Tender, 10, 15),
            ],
        );
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: 11,
            y: 11,
        });
        let mut state = scenario.build().unwrap();
        let attacker = state.units()[0].id;
        let victim = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Fabricator)
            .unwrap()
            .id;
        let mut hits = vec![];
        for tick in 0..350 {
            let commands = if tick == 0 {
                vec![cmd(
                    0,
                    Command::Attack {
                        units: vec![attacker],
                        target: Target::Building(victim).into(),
                        queue: false,
                    },
                )]
            } else {
                vec![]
            };
            let report = state.tick(&commands);
            for event in report.events {
                if let Event::AttackHit {
                    attacker: id,
                    attacker_pos,
                    target_pos,
                    ..
                } = event
                    && id == attacker
                {
                    assert!(
                        attacker_pos.dist(target_pos) <= Fx::lit("0.441"),
                        "hit across a gap from {start:?}"
                    );
                    hits.push(tick);
                }
            }
            if hits.len() == 4 {
                break;
            }
        }
        assert_eq!(hits.len(), 4, "failed to close from {start:?}");
        assert!(hits.windows(2).all(|pair| pair[1] - pair[0] == 6));
    }
}

#[test]
fn scuttler_can_bite_small_and_large_chassis_without_overlapping_them() {
    for kind in [
        UnitKind::Harvester,
        UnitKind::Excavator,
        UnitKind::Avalanche,
    ] {
        let mut state = open_arena(
            28,
            20,
            vec![unit(0, UnitKind::Scuttler, 8, 8), unit(1, kind, 9, 8)],
        )
        .build()
        .unwrap();
        let attacker = state.units()[0].id;
        let victim = state.units()[1].id;
        let mut wire = serde_json::to_value(&state).unwrap();
        for (slot, weapon) in kind.stats().weapons.iter().enumerate() {
            wire["units"][1]["cooldowns"][slot] = serde_json::json!(weapon.cooldown_ticks);
        }
        state = serde_json::from_value(wire).unwrap();
        let mut struck = false;
        for tick in 0..100 {
            let commands = if tick == 0 {
                vec![cmd(
                    0,
                    Command::Attack {
                        units: vec![attacker],
                        target: Target::Unit(victim).into(),
                        queue: false,
                    },
                )]
            } else {
                vec![]
            };
            let report = state.tick(&commands);
            if let Some(distance) = report.events.iter().find_map(|event| match event {
                Event::AttackHit {
                    attacker: id,
                    attacker_pos,
                    target_pos,
                    ..
                } if *id == attacker => Some(attacker_pos.dist(*target_pos)),
                _ => None,
            }) {
                let reach =
                    (UnitKind::Scuttler.stats().radius + kind.stats().radius + Fx::lit("0.16"))
                        .min(Fx::lit("0.8"));
                assert!(distance <= reach + Fx::lit("0.001"));
                assert!(distance > Fx::lit("0.4"));
                struck = true;
                break;
            }
        }
        assert!(struck, "never contacted {kind:?}");
    }
}
