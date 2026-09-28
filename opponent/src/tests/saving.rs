use super::*;
use crate::investments::{self, Candidate, Investment, Situation};
use crate::memory::Memory;
use crate::saving::Saving;
use crate::{PersonalityTraits, Step};

/// The arena with both seats' Harvesters saturating their two nodes, so a
/// Fabricator is worth saving for from the first decision.
fn saturated(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.units.extend([
        harvester(0, 5, 7),
        harvester(0, 4, 7),
        harvester(1, 18, 4),
        harvester(1, 19, 4),
    ]);
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
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
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
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
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
    let mut checkpoint = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
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
    let mut opponent = seat(&scenario, 0);
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
    let mut opponent = seat(&scenario, 0);
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
    let mut opponent = seat(&scenario, 0);
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
    let west = builds(&seat(&scenario, 0).act(&state, &mut OwnEvents::default()));
    let east = builds(&seat(&scenario, 1).act(&state, &mut OwnEvents::default()));
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
            spendable: 0,
            pull: Vec::new(),
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
    assert!(wanted.contains(&Investment::Refinery(reclaimer)));
    assert_eq!(
        investments::step(&observation, Investment::Refinery(reclaimer)),
        Some((Step::Build(BuildingKind::Fabricator), 120))
    );
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
