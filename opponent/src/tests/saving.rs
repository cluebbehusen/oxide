use super::*;
use crate::defenses;
use crate::investments::{self, ADOPT, Candidate, Investment, Situation};
use crate::memory::Memory;
use crate::saving::Saving;
use crate::{PersonalityTraits, Step};
use oxide_sim::scenario::BotDifficulty;

/// The arena with both seats' Harvesters saturating their nodes, so a
/// Fabricator is worth saving for from the first decision.
fn saturated(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.units.extend(workforce(0));
    scenario.units.extend(workforce(1));
    scenario.units.extend(standing_army(0));
    scenario.units.extend(standing_army(1));
    scenario
}

fn builds(commands: &[PlayerCommand]) -> Vec<(BuildingKind, TilePos)> {
    commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Build { kind, anchor, .. } => Some((kind, anchor)),
            _ => None,
        })
        .collect()
}

fn traits() -> PersonalityTraits {
    PersonalityTraits {
        air: 50,
        siege: 50,
        support: 50,
        fortification: 50,
        greed: 50,
        guile: 50,
    }
}

#[test]
fn an_affordable_target_is_bought_before_production_spends_the_rest() {
    let scenario = saturated(400);
    let state = scenario.build().unwrap();
    let (commands, trace) =
        seat_with(&scenario, 0, thrifty()).act_traced(&state, &mut OwnEvents::default());
    let trace = trace.unwrap();
    assert_eq!(
        trace
            .target
            .and_then(|target| target.next)
            .map(|next| next.step),
        Some(Step::Build(BuildingKind::Fabricator)),
        "every first tech target starts with a Fabricator"
    );
    assert_eq!(builds(&commands).len(), 1);
    assert_eq!(builds(&commands)[0].0, BuildingKind::Fabricator);
    let foundry = foundries(&state, PlayerId(0))[0];
    assert_eq!(trains(&commands), [(foundry, UnitKind::Sentinel)]);
    assert_eq!(trace.protected, 0, "the purchase spent what was protected");
}

#[test]
fn production_never_spends_scrap_protected_for_the_target() {
    let scenario = saturated(100);
    let state = scenario.build().unwrap();
    let (commands, trace) =
        seat_with(&scenario, 0, thrifty()).act_traced(&state, &mut OwnEvents::default());
    let trace = trace.unwrap();
    assert!(trace.protected > 0);
    assert!(trace.bank >= UnitKind::Sentinel.stats().cost, "premise");
    assert!(trains(&commands).is_empty(), "{commands:?}");
    assert!(builds(&commands).is_empty(), "the target is not affordable");
}

#[test]
fn a_seat_without_harvesters_spends_protected_scrap_on_one() {
    let mut scenario = arena(60);
    scenario.units.retain(|unit| unit.player != 0);
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, 1_800, &[]);
    let bank = state.player(PlayerId(0)).scrap;
    let protected = bank.min(120);
    assert!(
        bank - protected < UnitKind::Harvester.stats().cost,
        "premise"
    );
    let mut checkpoint =
        serde_json::to_value(seat_with(&scenario, 0, thrifty()).checkpoint()).unwrap();
    checkpoint["saving"] = serde_json::json!({
        "protected": protected,
        "target": {"investment": {"tech": "fabricator"}, "attempt": null},
    });
    let checkpoint: Checkpoint = serde_json::from_value(checkpoint).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let foundry = foundries(&state, PlayerId(0))[0];
    assert_eq!(trains(&commands), [(foundry, UnitKind::Harvester)]);
    let left = bank - UnitKind::Harvester.stats().cost;
    assert_eq!(trace.unwrap().protected, protected.min(left));
}

#[test]
fn a_missing_purchase_keeps_the_target_and_tries_another_spot() {
    let scenario = saturated(400);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat_with(&scenario, 0, thrifty());
    let mut events = OwnEvents::default();
    let (commands, trace) = opponent.act_traced(&state, &mut events);
    let first = builds(&commands);
    let target = trace.unwrap().target.map(|target| target.investment);
    assert!(target.is_some());
    assert_eq!(first.len(), 1);
    events.record(
        PlayerId(0),
        &[Event::CommandRejected {
            player: PlayerId(0),
            reason: RejectReason::BadSite,
        }],
    );
    advance_to(&mut state, 12, &[]);
    let (commands, trace) = opponent.act_traced(&state, &mut events);
    let second = builds(&commands);
    assert_eq!(
        trace.unwrap().target.map(|target| target.investment),
        target
    );
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, first[0].0);
    assert_ne!(second[0].1, first[0].1);
}

#[test]
fn a_cancelled_site_is_bought_again_elsewhere() {
    let scenario = saturated(400);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat_with(&scenario, 0, thrifty());
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let first = builds(&commands);
    state.tick(&commands);
    let site = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0) && building.kind == BuildingKind::Fabricator
        })
        .expect("the site was placed")
        .id;
    let cancel = PlayerCommand {
        player: PlayerId(0),
        command: Command::Cancel { building: site },
    };
    advance_to(&mut state, 12, &[cancel]);
    let second = builds(&opponent.act(&state, &mut OwnEvents::default()));
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].0, BuildingKind::Fabricator);
    assert_ne!(second[0].1, first[0].1);
}

#[test]
fn an_unattended_site_gets_a_builder() {
    let scenario = saturated(400);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat_with(&scenario, 0, thrifty());
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let builder = commands
        .iter()
        .find_map(|command| match &command.command {
            Command::Build { units, .. } => Some(units[0]),
            _ => None,
        })
        .unwrap();
    state.tick(&commands);
    let hp = |state: &State| {
        ObservationData::fog_honest(state, PlayerId(0))
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Fabricator)
            .map(|building| building.hp)
    };
    let placed = hp(&state).unwrap();
    for _ in 0..600 {
        if hp(&state).unwrap() > placed {
            break;
        }
        state.tick(&[]);
    }
    assert!(hp(&state).unwrap() > placed, "premise: the builder started");
    let stop = PlayerCommand {
        player: PlayerId(0),
        command: Command::Stop {
            units: vec![builder],
        },
    };
    let next = state.current_tick().next_multiple_of(12);
    advance_to(&mut state, next, &[stop]);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert_eq!(
        builds(&commands),
        [(BuildingKind::Fabricator, first_site(&state))]
    );
    assert!(
        trace
            .unwrap()
            .purchases
            .iter()
            .all(|purchase| !matches!(purchase, Purchase::Build { .. })),
        "resuming costs nothing"
    );
}

fn first_site(state: &State) -> TilePos {
    state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0) && building.kind == BuildingKind::Fabricator
        })
        .unwrap()
        .anchor
}

#[test]
fn mirrored_seats_build_on_mirrored_spots() {
    let scenario = saturated(400);
    let state = scenario.build().unwrap();
    let west = builds(&seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default()));
    let east = builds(&seat_with(&scenario, 1, thrifty()).act(&state, &mut OwnEvents::default()));
    let (width, height) = (state.map().width(), state.map().height());
    assert_eq!(west.len(), 1);
    assert_eq!(
        east,
        west.iter()
            .map(|(kind, anchor)| (
                *kind,
                TilePos::new(width - 2 - anchor.x, height - 2 - anchor.y)
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn scrapheap_still_techs() {
    let scrapheap = BotConfig::opponent(BotDifficulty::Scrapheap, BotStance::Balanced, 11);
    let scenario = arena(200);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat_with(&scenario, 0, scrapheap);
    let mut events = OwnEvents::default();
    while state.current_tick() < 3_600 && foundries_of(&state, BuildingKind::Fabricator) == 0 {
        let commands = opponent.act(&state, &mut events);
        events.record(PlayerId(0), &state.tick(&commands).events);
    }
    assert_eq!(foundries_of(&state, BuildingKind::Fabricator), 1);
}

fn foundries_of(state: &State, kind: BuildingKind) -> usize {
    state
        .buildings()
        .iter()
        .filter(|building| building.player == PlayerId(0) && building.kind == kind)
        .count()
}

#[test]
fn targets_switch_only_for_a_clearly_better_investment() {
    let state = saturated(200).build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let candidate = |investment, score| Candidate { investment, score };
    let fabricator = Investment::Tech(BuildingKind::Fabricator);
    let mut memory = Memory::default();
    let mut saving = Saving::default();

    saving.settle(
        &observation,
        &[candidate(fabricator, 400)],
        500,
        0,
        &mut memory,
    );
    assert_eq!(saving.investment(), Some(fabricator));
    assert_eq!(saving.protected(), 100, "half the bank on adoption");

    let near = [
        candidate(Investment::Reclaimer, 549),
        candidate(fabricator, 400),
    ];
    saving.settle(&observation, &near, 500, 20, &mut memory);
    assert_eq!(saving.investment(), Some(fabricator), "a near tie holds");
    assert_eq!(saving.protected(), 110, "half of the earnings join it");

    let clear = [
        candidate(Investment::Reclaimer, 550),
        candidate(fabricator, 400),
    ];
    saving.settle(&observation, &clear, 500, 0, &mut memory);
    assert_eq!(saving.investment(), Some(Investment::Reclaimer));
    assert_eq!(saving.protected(), 110, "protected scrap carries over");

    saving.settle(
        &observation,
        &[candidate(fabricator, 100)],
        500,
        0,
        &mut memory,
    );
    assert_eq!(
        saving.investment(),
        None,
        "the reason is gone and nothing new is worth it"
    );
    assert_eq!(saving.protected(), 0);
}

#[test]
fn reclaimers_wait_for_the_drip_and_refineries_need_a_fabricator() {
    let mut scenario = saturated(200);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Reclaimer,
        x: 8,
        y: 8,
    });
    let state = scenario.build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let model = map(&scenario);
    let memory = Memory::default();
    let wants = |observation: &ObservationData| {
        investments::candidates(&Situation {
            observation,
            map: &model,
            memory: &memory,
            traits: traits(),
            saturation: 1_000,
            income: 400,
            depletion: 0,
            pull: Vec::new(),
            exposed: false,
            stakes: defenses::Stakes::default(),
            severed: false,
            wanted: Vec::new(),
            units: Vec::new(),
            waiting: None,
        })
        .into_iter()
        .map(|candidate| candidate.investment)
        .collect::<Vec<_>>()
    };
    assert!(!wants(&observation).contains(&Investment::Reclaimer));
    observation.tick = 2_400;
    let wanted = wants(&observation);
    assert!(wanted.contains(&Investment::Reclaimer));
    let reclaimer = observation
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Reclaimer)
        .unwrap()
        .id;
    assert!(wanted.contains(&Investment::Upgrade {
        building: reclaimer,
        tier: 1
    }));
    assert_eq!(
        investments::step(
            &observation,
            Investment::Upgrade {
                building: reclaimer,
                tier: 1
            }
        ),
        Some((Step::Build(BuildingKind::Fabricator), 120))
    );
}

#[test]
fn reclaimers_keep_coming_once_the_home_scrap_is_mined_out() {
    let mut scenario = saturated(200);
    for x in 6..12 {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x,
            y: 8,
        });
    }
    let state = scenario.build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    observation.tick = 2_400;
    let model = map(&scenario);
    let memory = Memory::default();
    let score = |depletion: u32| {
        investments::candidates(&Situation {
            observation: &observation,
            map: &model,
            memory: &memory,
            traits: traits(),
            saturation: 1_000,
            income: 400,
            depletion,
            pull: Vec::new(),
            exposed: false,
            stakes: defenses::Stakes::default(),
            severed: false,
            wanted: Vec::new(),
            units: Vec::new(),
            waiting: None,
        })
        .into_iter()
        .find(|candidate| candidate.investment == Investment::Reclaimer)
        .map_or(0, |candidate| candidate.score)
    };
    assert!(
        score(0) < ADOPT,
        "six Reclaimers are plenty beside full nodes"
    );
    assert!(score(1_000) >= ADOPT, "but not once the nodes are gone");
}

/// The saturated arena with `scrap`, a West Crucible, West Lancers making up
/// its siege, and a West Kestrel in sight of East's army grown to outweigh
/// West's line, out of reach of West's buildings.
fn outlined(scrap: u32) -> Scenario {
    let mut scenario = saturated(scrap);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Crucible,
        x: 5,
        y: 7,
    });
    // West's line keeps just the Aggressive minimum army.
    scenario
        .units
        .retain(|unit| !(unit.player == 0 && unit.kind == UnitKind::Sentinel && unit.x >= 8));
    scenario.units.push(unit(0, UnitKind::Kestrel, 12, 6));
    for x in 3..=7 {
        scenario.units.push(unit(0, UnitKind::Lancer, x, 1));
    }
    for x in 15..=20 {
        for y in 3..=4 {
            scenario.units.push(unit(1, UnitKind::Sentinel, x, y));
        }
    }
    scenario
}

fn crucible(state: &State) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Crucible)
        .unwrap()
        .id
}

#[test]
fn a_seat_saves_for_the_dear_unit_its_line_prefers_and_trains_it() {
    let scenario = outlined(800);
    let state = scenario.build().unwrap();
    let mut opponent = seat_with(&scenario, 0, thrifty());
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let trace = trace.unwrap();
    assert_eq!(
        trace.target.map(|target| target.investment),
        Some(Investment::Unit(UnitKind::Breaker)),
        "{trace:?}"
    );
    assert!(trace.protected > 0);
    assert!(
        !trains(&commands)
            .iter()
            .any(|(_, kind)| *kind == UnitKind::Breaker),
        "premise: not yet affordable"
    );
    assert!(
        !trains(&commands).iter().any(|(building, kind)| {
            *building == crucible(&state)
                || crate::composition::role(*kind) == Some(crate::composition::Role::Line)
        }),
        "the Crucible waits, and no cheaper line unit takes the scrap: {commands:?}"
    );
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    assert_eq!(
        json["saving"]["target"]["investment"],
        serde_json::json!({"unit": "breaker"})
    );
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let richer = outlined(1_400);
    let mut state = richer.build().unwrap();
    let interval = crate::decision_interval(BotDifficulty::Standard);
    advance_to(&mut state, interval, &[]);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        trains(&commands).contains(&(crucible(&state), UnitKind::Breaker)),
        "{commands:?}"
    );
    assert_eq!(trace.unwrap().protected, 0, "the purchase spent it");
}

#[test]
fn a_producer_saved_for_queues_nothing_else_first() {
    // Line wanted against East's army out of reach of West's buildings, the
    // Fabricator's Warden its dear unit, and a wounded West army that would
    // otherwise call for a Tender there.
    let mut scenario = saturated(200);
    scenario
        .units
        .retain(|unit| unit.kind != UnitKind::Sentinel || (unit.player == 0 && unit.x < 8));
    scenario.units.push(unit(0, UnitKind::Kestrel, 15, 4));
    scenario
        .units
        .extend((9..=13).map(|x| unit(0, UnitKind::Sentinel, x, 9)));
    for x in 17..=21 {
        for y in 1..=4 {
            scenario.units.push(unit(1, UnitKind::Sentinel, x, y));
        }
    }
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 5,
        y: 7,
    });
    let mut state = scenario.build().unwrap();
    let hurt: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    for unit in hurt {
        state = wounded(&state, unit, 1);
    }
    let fabricator = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Fabricator)
        .unwrap()
        .id;
    let (commands, trace) =
        seat_with(&scenario, 0, thrifty()).act_traced(&state, &mut OwnEvents::default());
    let trace = trace.unwrap();
    assert_eq!(
        trace.target.map(|target| target.investment),
        Some(Investment::Unit(UnitKind::Warden)),
        "premise: {trace:?}"
    );
    assert!(
        !trains(&commands)
            .iter()
            .any(|(building, _)| *building == fabricator),
        "{commands:?}"
    );
}

#[test]
fn a_unit_saved_for_that_waits_on_busy_producers_wants_another() {
    let scenario = outlined(1_400);
    let mut state = scenario.build().unwrap();
    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: crucible(&state),
            kind: UnitKind::Avalanche,
        },
    }]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let model = map(&scenario);
    let memory = Memory::default();
    let wants = |waiting: Option<BuildingKind>| {
        investments::candidates(&Situation {
            observation: &observation,
            map: &model,
            memory: &memory,
            traits: traits(),
            saturation: 1_000,
            income: 400,
            depletion: 0,
            pull: Vec::new(),
            exposed: false,
            stakes: defenses::Stakes::default(),
            severed: false,
            wanted: Vec::new(),
            units: Vec::new(),
            waiting,
        })
        .into_iter()
        .map(|candidate| candidate.investment)
        .collect::<Vec<_>>()
    };
    let another = Investment::Capacity(BuildingKind::Crucible);
    assert!(
        !wants(None).contains(&another),
        "premise: no role asks for it"
    );
    assert!(wants(Some(BuildingKind::Crucible)).contains(&another));
}

#[test]
fn opponents_invest_without_rejected_orders() {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = true;
        seat.bot_config = Some(config());
    }
    let mut state = scenario.build().unwrap();
    let mut seats = [seat(&scenario, 0), seat(&scenario, 1)];
    while state.current_tick() < 4_800 {
        let commands: Vec<_> = seats
            .iter_mut()
            .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
            .collect();
        let report = state.tick(&commands);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "{:?}",
            report.events
        );
    }
    for player in [PlayerId(0), PlayerId(1)] {
        let owned: Vec<BuildingKind> = state
            .buildings()
            .iter()
            .filter(|building| building.player == player)
            .map(|building| building.kind)
            .collect();
        assert!(
            owned.contains(&BuildingKind::Fabricator),
            "{player:?} {owned:?}"
        );
        assert!(
            owned
                .iter()
                .any(|kind| !matches!(kind, BuildingKind::Foundry | BuildingKind::Fabricator)),
            "{player:?} invested beyond its first tech: {owned:?}"
        );
    }
}

#[test]
fn a_restored_seat_continues_its_saving_exactly() {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = true;
        seat.bot_config = Some(config());
    }
    let mut state = scenario.build().unwrap();
    let mut seats = [seat(&scenario, 0), seat(&scenario, 1)];
    let mut protected = false;
    while state.current_tick() < 1_500 {
        let commands: Vec<_> = seats
            .iter_mut()
            .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
            .collect();
        state.tick(&commands);
        protected |= seats.iter().any(|seat| seat.protected_scrap() > 0);
    }
    assert!(protected, "premise: the run saved for something");
    let mut restored: Vec<Opponent> = seats
        .iter()
        .map(|seat| {
            let json = serde_json::to_string(&seat.checkpoint()).unwrap();
            let checkpoint: Checkpoint = serde_json::from_str(&json).unwrap();
            Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap()
        })
        .collect();
    for _ in 0..600 {
        let original: Vec<_> = seats
            .iter_mut()
            .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
            .collect();
        let resumed: Vec<_> = restored
            .iter_mut()
            .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
            .collect();
        assert_eq!(original, resumed);
        state.tick(&original);
    }
}

#[test]
fn a_scaffold_stays_pending_until_its_ground_is_confirmed() {
    let state = saturated(400).build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let fabricator = Investment::Tech(BuildingKind::Fabricator);
    let candidates = [Candidate {
        investment: fabricator,
        score: 400,
    }];
    let mut memory = Memory::default();
    let mut saving = Saving::default();
    saving.settle(&observation, &candidates, 500, 0, &mut memory);
    let anchor = TilePos::new(9, 8);
    saving.attempted(Step::Build(BuildingKind::Fabricator), anchor);

    let mut scaffold = observation.my_buildings[0].clone();
    scaffold.id = BuildingId(99);
    scaffold.kind = BuildingKind::Fabricator;
    scaffold.anchor = anchor;
    scaffold.built = false;
    scaffold.provisional = true;
    observation.my_buildings.push(scaffold);
    observation.tick = 12;
    saving.settle(&observation, &candidates, 500, 0, &mut memory);
    assert!(saving.pending(), "a scaffold is not yet a site");
    assert_eq!(saving.investment(), Some(fabricator));

    observation.my_buildings.pop();
    observation.tick = 24;
    saving.settle(&observation, &candidates, 500, 0, &mut memory);
    assert!(!saving.pending());
    assert!(
        memory.failed(BuildingKind::Fabricator, anchor, 24),
        "revealed and refunded"
    );
    assert_eq!(saving.investment(), Some(fabricator), "the target stays");
}

#[test]
fn a_harvester_sealed_off_from_the_site_never_builds_it() {
    let mut scenario = arena(400);
    scenario.map[7].replace_range(5..8, "###");
    scenario.map[8].replace_range(5..8, "#.#");
    scenario.map[9].replace_range(5..8, "###");
    let sealed = harvester(0, 6, 8);
    scenario.units = vec![
        sealed,
        harvester(0, 12, 3),
        harvester(0, 13, 3),
        harvester(0, 12, 4),
        harvester(1, 16, 5),
        harvester(1, 17, 5),
    ];
    let state = scenario.build().unwrap();
    let sealed = state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(sealed.x, sealed.y))
        .unwrap()
        .id;
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let (builders, anchor) = commands
        .iter()
        .find_map(|command| match &command.command {
            Command::Build { units, anchor, .. } => Some((units.clone(), *anchor)),
            _ => None,
        })
        .expect("the target is bought");
    let nearest = |unit: &oxide_sim::state::Unit| unit.tile().chebyshev(anchor);
    let sealed_distance = nearest(state.unit(sealed).unwrap());
    assert!(
        state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0) && unit.id != sealed)
            .all(|unit| nearest(unit) > sealed_distance),
        "premise: the sealed Harvester is nearest the site"
    );
    assert!(!builders.contains(&sealed));
}

#[test]
fn mirrored_seats_rank_equal_extractor_frames_alike() {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = true;
        seat.bot_config = Some(config());
    }
    let state = scenario.build().unwrap();
    let model = map(&scenario);
    let memory = Memory::default();
    let first_extractor = |player: u8| {
        let observation = ObservationData::fog_honest(&state, PlayerId(player));
        investments::candidates(&Situation {
            observation: &observation,
            map: &model,
            memory: &memory,
            traits: traits(),
            saturation: 1_000,
            income: 400,
            depletion: 0,
            pull: Vec::new(),
            exposed: false,
            stakes: defenses::Stakes::default(),
            severed: false,
            wanted: Vec::new(),
            units: Vec::new(),
            waiting: None,
        })
        .into_iter()
        .find_map(|candidate| match candidate.investment {
            Investment::Extractor(frame) => Some(frame),
            _ => None,
        })
        .expect("a frame is in reach")
    };
    let (width, height) = (state.map().width(), state.map().height());
    let west = first_extractor(0);
    assert_eq!(
        first_extractor(1),
        TilePos::new(width - 2 - west.x, height - 2 - west.y)
    );
}

#[test]
fn a_building_with_nowhere_to_stand_is_not_saved_for() {
    for bank in [130, 100] {
        let (commands, trace, foundry) = nowhere_to_stand(bank);
        assert!(builds(&commands).is_empty(), "premise: no spot can take it");
        assert!(
            !matches!(
                trace.target.map(|target| target.investment),
                Some(Investment::Tech(_) | Investment::Capacity(_) | Investment::Reclaimer)
            ),
            "bank {bank}: {trace:?}"
        );
        assert!(
            trains(&commands)
                .iter()
                .any(|(building, _)| *building == foundry),
            "production goes on: {commands:?}"
        );
    }
}

/// The first decision of a saturated seat with `bank` scrap whose every home
/// spot is taken.
fn nowhere_to_stand(bank: u32) -> (Vec<PlayerCommand>, Trace, BuildingId) {
    let mut scenario = saturated(bank);
    let model = map(&scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let spots: Vec<TilePos> = model.spots(PlayerId(0), vec![start]).collect();
    for anchor in spots {
        if occupied(&scenario, anchor) {
            continue;
        }
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: anchor.x,
            y: anchor.y,
        });
    }
    let state = scenario.build().unwrap();
    let (commands, trace) =
        seat_with(&scenario, 0, thrifty()).act_traced(&state, &mut OwnEvents::default());
    (commands, trace.unwrap(), foundries(&state, PlayerId(0))[0])
}

/// Whether a unit or building of `scenario` stands on `tile`.
fn occupied(scenario: &Scenario, tile: TilePos) -> bool {
    let state = scenario.build().unwrap();
    scenario
        .units
        .iter()
        .any(|unit| TilePos::new(unit.x, unit.y) == tile)
        || state.buildings().iter().any(|building| {
            let (width, height) = building.kind.base_stats().size;
            (building.anchor.x..building.anchor.x + width).contains(&tile.x)
                && (building.anchor.y..building.anchor.y + height).contains(&tile.y)
        })
}

#[test]
fn a_seat_whose_home_is_full_builds_beside_its_other_foundry() {
    let mut scenario = saturated(400);
    let expansion = TilePos::new(10, 8);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: expansion.x,
        y: expansion.y,
    });
    let model = map(&scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let distance = |a: TilePos, b: TilePos| (a.x - b.x).abs().max((a.y - b.y).abs());
    let home: Vec<TilePos> = model
        .spots(PlayerId(0), vec![start, expansion])
        .filter(|anchor| distance(*anchor, start) < distance(*anchor, expansion))
        .collect();
    for anchor in home {
        if occupied(&scenario, anchor) {
            continue;
        }
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: anchor.x,
            y: anchor.y,
        });
    }
    let state = scenario.build().unwrap();
    let (commands, trace) =
        seat_with(&scenario, 0, thrifty()).act_traced(&state, &mut OwnEvents::default());
    let homes: Vec<(BuildingKind, TilePos)> = builds(&commands)
        .into_iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                BuildingKind::Fabricator
                    | BuildingKind::Airworks
                    | BuildingKind::Crucible
                    | BuildingKind::Reclaimer
            )
        })
        .collect();
    assert!(!homes.is_empty(), "{trace:?}");
    // The nearest spot an expansion Foundry leaves room for.
    let beside = model
        .spots(PlayerId(0), vec![expansion])
        .map(|anchor| distance(anchor, expansion))
        .next()
        .unwrap();
    for (kind, anchor) in homes {
        assert_eq!(
            distance(anchor, expansion),
            beside,
            "{kind:?} at {anchor:?}"
        );
        assert!(distance(anchor, start) > beside, "{kind:?} at {anchor:?}");
    }
}

#[test]
fn no_building_goes_beside_a_foundry_no_worker_can_reach() {
    // Rock splits the field; West owns a Foundry across it, where only
    // East's workers stand.
    let mut scenario = arena(400);
    scenario.map = [
        "########################################",
        "#..................##..................#",
        "#..................##..................#",
        "#..................##..................#",
        "#..................##..................#",
        "#..1...............##...............2..#",
        "#..................##..................#",
        "#..................##..................#",
        "#..................##..................#",
        "#..................##..................#",
        "#..................##..................#",
        "########################################",
    ]
    .map(str::to_owned)
    .to_vec();
    scenario.units.clear();
    scenario.units.extend((1..=10).map(|y| harvester(0, 1, y)));
    scenario.units.extend((1..=10).map(|y| harvester(1, 38, y)));
    let across = TilePos::new(26, 5);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: across.x,
        y: across.y,
    });
    let state = scenario.build().unwrap();
    let model = map(&scenario);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let home = model.component(model.start(PlayerId(0)).unwrap());
    assert_ne!(model.component(across), home, "premise: across the rock");
    let spots: Vec<TilePos> = investments::anchors(
        &model,
        &observation,
        Investment::Tech(BuildingKind::Fabricator),
        BuildingKind::Fabricator,
    )
    .collect();
    assert!(!spots.is_empty());
    assert!(
        spots.iter().all(|spot| model.component(*spot) == home),
        "{:?}",
        spots.iter().find(|spot| model.component(**spot) != home)
    );
}

#[test]
fn checkpoints_reject_state_off_the_map_and_survive_extreme_samples() {
    let scenario = saturated(400);
    let state = scenario.build().unwrap();
    let json = serde_json::to_value(seat_with(&scenario, 0, thrifty()).checkpoint()).unwrap();
    let restore = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut json = json.clone();
        edit(&mut json);
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
    };
    assert_eq!(
        restore(&|json| {
            json["memory"]["units"] = serde_json::json!([
                {"id": 1, "kind": "sentinel", "tile": {"x": i32::MIN, "y": 0}, "seen": 0}
            ]);
        })
        .err()
        .unwrap(),
        "checkpoint enemy units are off the map"
    );
    assert_eq!(
        restore(&|json| {
            json["saving"]["target"] =
                serde_json::json!({"investment": {"extractor": {"x": 999, "y": 0}}, "attempt": null});
        })
        .err()
        .unwrap(),
        "checkpoint saving target is off the map"
    );
    assert_eq!(
        restore(&|json| json["missions"]["next"] = 1_000_000.into())
            .err()
            .unwrap(),
        "checkpoint mission ids are out of order"
    );
    let mut opponent = restore(&|json| {
        json["income"]["previous"] =
            serde_json::json!({"tick": 0, "bank": u32::MAX, "spent": u32::MAX});
    })
    .unwrap();
    let mut state = state.clone();
    advance_to(&mut state, 12, &[]);
    opponent.act(&state, &mut OwnEvents::default());
}
