use super::*;

fn needs(reach: Option<Reach>, clustered: bool) -> Needs {
    Needs {
        need: [0; 4],
        weight: [1_000; 4],
        ground: true,
        lifted: false,
        enemy: Enemy {
            air: 0,
            ground: 1_000,
            defenses: 0,
            reach,
            clustered,
            targets: Vec::new(),
        },
        traits: PersonalityTraits {
            air: 50,
            siege: 50,
            support: 50,
            fortification: 50,
            greed: 50,
            guile: 50,
        },
        income: 1_000,
        kinds: [0; UnitKind::ALL.len()],
    }
}

#[test]
fn a_seat_wants_siege_behind_its_line_by_stance_unless_it_lifts_its_army() {
    use oxide_sim::{PlayerId, Scenario};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_units[0].clone();
    observation
        .my_units
        .extend((0..8).map(|_| oxide_sim::observation::UnitObs {
            kind: UnitKind::Sentinel,
            ..template.clone()
        }));
    let memory = Memory::default();
    let siege = |stance, lifted| {
        let outlet = Outlet {
            ground: true,
            lifted,
            invaders: 0,
            air_strikes: false,
            strike: 0,
        };
        super::needs(
            &observation,
            &memory,
            needs(None, false).traits,
            stance,
            0,
            outlet,
        )
        .need[Role::Siege as usize]
    };
    assert!(siege(BotStance::Balanced, false) > 0);
    assert!(siege(BotStance::Aggressive, false) < siege(BotStance::Balanced, false));
    assert!(
        siege(BotStance::Balanced, true) <= 0,
        "no known defenses call for siege a Skyhook must carry"
    );
}

#[test]
fn longer_reaching_enemies_count_against_short_reach() {
    let sentinel = |reach| needs(Some(reach), false).suitability(UnitKind::Sentinel, Role::Line);
    assert!(sentinel(Reach::Long) < sentinel(Reach::Medium));
    assert!(sentinel(Reach::Medium) < sentinel(Reach::Short));
}

#[test]
fn clustered_enemies_favour_splash() {
    let prefers = |clustered| {
        let needs = needs(None, clustered);
        needs.suitability(UnitKind::Bombard, Role::Siege)
            > needs.suitability(UnitKind::Lancer, Role::Siege)
    };
    assert!(prefers(true));
    assert!(!prefers(false));
}

#[test]
fn clustering_reads_the_most_recently_seen_enemies() {
    use oxide_sim::observation::UnitObs;
    use oxide_sim::{PlayerId, Scenario, UnitId};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_units[0].clone();
    let unit = |id: u32, x: i32, y: i32| UnitObs {
        id: UnitId(id),
        kind: UnitKind::Sentinel,
        tile: chassis::grid::TilePos::new(x, y),
        ..template.clone()
    };
    observation
        .visible
        .iter_mut()
        .for_each(|visible| *visible = false);
    observation.enemy_units = (0..36)
        .map(|index| {
            unit(
                100 + index,
                (index % 6).cast_signed() * 6,
                (index / 6).cast_signed() * 4,
            )
        })
        .collect();
    let mut memory = Memory::default();
    memory.observe(&observation);
    assert!(
        !Enemy::of(&observation, &memory).clustered,
        "premise: spread out"
    );
    observation.tick = 300;
    observation.enemy_units = (1..=4)
        .map(|index| {
            unit(
                index,
                20 + (index % 2).cast_signed(),
                20 + (index / 2).cast_signed(),
            )
        })
        .collect();
    memory.observe(&observation);
    assert!(Enemy::of(&observation, &memory).clustered);
}

#[test]
fn a_dear_unit_the_role_prefers_is_worth_saving_for_while_a_cheaper_one_fits() {
    use oxide_sim::observation::BuildingObs;
    use oxide_sim::{BuildingId, PlayerId, Scenario};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_buildings[0].clone();
    for (id, kind) in [
        (900, BuildingKind::Fabricator),
        (901, BuildingKind::Crucible),
    ] {
        observation.my_buildings.push(BuildingObs {
            id: BuildingId(id),
            kind,
            built: true,
            ..template.clone()
        });
    }
    let mut needs = needs(None, false);
    needs.income = 3_000;
    needs.need[Role::Siege as usize] = 2_000;
    // Lancers and Bombards already make up the siege.
    for (kind, count) in [(UnitKind::Lancer, 6), (UnitKind::Bombard, 4)] {
        needs.kinds[index(kind)] = count;
    }
    let premium = needs.premium(&observation, 150);
    let [(kind, score)] = premium[..] else {
        panic!("{premium:?}");
    };
    assert_eq!(kind, UnitKind::Avalanche);
    assert_eq!(u64::from(score), needs.weight[Role::Siege as usize]);
    assert!(
        needs.premium(&observation, 10_000).is_empty(),
        "affordable now"
    );
    needs.need[Role::Siege as usize] = 0;
    assert!(needs.premium(&observation, 150).is_empty(), "not wanted");
}

#[test]
fn a_short_role_whose_best_unit_needs_a_crucible_pulls_toward_one() {
    use oxide_sim::observation::BuildingObs;
    use oxide_sim::{BuildingId, PlayerId, Scenario};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_buildings[0].clone();
    observation.my_buildings.push(BuildingObs {
        id: BuildingId(900),
        kind: BuildingKind::Fabricator,
        built: true,
        ..template.clone()
    });
    let mut needs = needs(None, false);
    needs.income = 3_000;
    needs.need[Role::Siege as usize] = 2_000;
    // Lancers and Bombards already make up the siege.
    for (kind, count) in [(UnitKind::Lancer, 6), (UnitKind::Bombard, 4)] {
        needs.kinds[index(kind)] = count;
    }
    let crucible = |pull: &[(BuildingKind, u32)]| {
        pull.iter()
            .find(|(building, _)| *building == BuildingKind::Crucible)
            .map(|(_, score)| *score)
    };
    assert_eq!(
        crucible(&needs.pull(&observation)),
        Some(needs.worth(Role::Siege, UnitKind::Avalanche))
    );
    observation.my_buildings.push(BuildingObs {
        id: BuildingId(901),
        kind: BuildingKind::Crucible,
        built: false,
        ..template.clone()
    });
    assert_eq!(crucible(&needs.pull(&observation)), None, "one is coming");
    observation.my_buildings.pop();
    needs.need[Role::Siege as usize] = 0;
    assert_eq!(crucible(&needs.pull(&observation)), None, "not wanted");
}

/// Six Flakhounds packed on two rows, at full health.
fn clump() -> Vec<(TilePos, u32)> {
    (0..6)
        .map(|index| {
            (
                TilePos::new(20 + index % 3, 20 + index / 3),
                UnitKind::Flakhound.stats().max_hp,
            )
        })
        .collect()
}

#[test]
fn a_seat_that_can_only_lift_scores_bombers_by_the_clump_one_blast_takes() {
    let condor = |lifted, targets: Vec<(TilePos, u32)>| {
        let mut needs = needs(None, false);
        needs.lifted = lifted;
        needs.enemy.targets = targets;
        needs.suitability(UnitKind::Condor, Role::AirStrike)
    };
    let clumped = condor(true, clump());
    let spread = clump()
        .into_iter()
        .map(|(tile, hp)| (TilePos::new(tile.x * 8, tile.y), hp))
        .collect();
    assert!(condor(true, spread) < clumped, "one blast, one Flakhound");
    assert!(
        condor(false, clump()) < clumped,
        "a seat whose ground reaches the enemy weighs splash as before"
    );
}

/// Skirmish's west seat with its Foundry and `producers`.
fn producing(producers: &[BuildingKind]) -> ObservationData {
    use oxide_sim::observation::BuildingObs;
    use oxide_sim::{BuildingId, PlayerId, Scenario};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_buildings[0].clone();
    for (id, kind) in (900..).zip(producers) {
        observation.my_buildings.push(BuildingObs {
            id: BuildingId(id),
            kind: *kind,
            built: true,
            ..template.clone()
        });
    }
    observation
}

/// Needs short of strike aircraft against `clump`.
fn striking(lifted: bool) -> Needs {
    let mut needs = needs(None, false);
    needs.lifted = lifted;
    needs.enemy.targets = clump();
    needs.income = 3_000;
    needs.need[Role::AirStrike as usize] = 2_000;
    needs
}

#[test]
fn a_seat_that_can_only_lift_saves_the_more_for_a_bomber_the_more_its_blast_takes() {
    let observation = producing(&[BuildingKind::Airworks, BuildingKind::Crucible]);
    let (_, saved) = striking(true)
        .premium(&observation, UnitKind::Buzzard.stats().cost)
        .into_iter()
        .find(|(kind, _)| *kind == UnitKind::Condor)
        .expect("worth saving for");
    assert!(
        saved > striking(true).worth(Role::AirStrike, UnitKind::Condor),
        "more than the role's weight alone"
    );
}

#[test]
fn a_seat_that_can_only_lift_pulls_toward_the_tech_its_bombers_need() {
    let observation = producing(&[BuildingKind::Airworks]);
    let crucible = |lifted| {
        striking(lifted)
            .pull(&observation)
            .into_iter()
            .any(|(building, _)| building == BuildingKind::Crucible)
    };
    assert!(crucible(true));
    assert!(!crucible(false));
}

#[test]
fn every_army_unit_gets_its_turn() {
    use oxide_sim::observation::BuildingObs;
    use oxide_sim::{BuildingId, PlayerId, Scenario};
    let state = Scenario::skirmish().build().unwrap();
    let producers = [
        BuildingKind::Foundry,
        BuildingKind::Fabricator,
        BuildingKind::Airworks,
        BuildingKind::Crucible,
    ];
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let template = observation.my_buildings[0].clone();
    for (id, kind) in (900..).zip(&producers[1..]) {
        observation.my_buildings.push(BuildingObs {
            id: BuildingId(id),
            kind: *kind,
            built: true,
            ..template.clone()
        });
    }
    let mut trained = Vec::new();
    for clustered in [false, true] {
        for role in ROLES {
            let mut needs = needs(None, clustered);
            needs.enemy.air = 1_000;
            needs.income = 3_000;
            needs.need[role as usize] = 100_000;
            for _ in 0..12 {
                for producer in producers {
                    if let Some(kind) = needs.unit(&observation, producer, role, 10_000) {
                        trained.push(kind);
                        needs.queued(kind);
                    }
                }
            }
        }
    }
    let missing: Vec<UnitKind> = UnitKind::ALL
        .into_iter()
        .filter(|kind| role(*kind).is_some())
        .filter(|kind| !trained.contains(kind))
        .collect();
    assert!(missing.is_empty(), "never trains {missing:?}");
}
