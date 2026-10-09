use super::*;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{AttackTarget, Command, Faction, PlayerCommand, PlayerId, Scenario};

/// A 32-wide, 12-deep field: West's start at (2, 2) and East's at
/// (28, 2), each seat with `scrap`, plus the given units and buildings.
fn field(
    scrap: u32,
    units: &[(u8, UnitKind, i32, i32)],
    buildings: &[(u8, BuildingKind, i32, i32)],
) -> Scenario {
    let ground = ".".repeat(32);
    let mut map = vec![ground.clone(); 12];
    let mut top: Vec<char> = ground.chars().collect();
    top[2] = '1';
    top[28] = '2';
    map[2] = top.into_iter().collect();
    Scenario {
        mode: ScenarioMode::Match,
        name: "ledger".into(),
        seed: 5,
        map,
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .enumerate()
            .map(|(seat, faction)| PlayerSpec {
                name: format!("seat {seat}"),
                faction,
                team: None,
                scrap,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: units
            .iter()
            .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
            .collect(),
        buildings: buildings
            .iter()
            .map(|&(player, kind, x, y)| BuildingSpec { player, kind, x, y })
            .collect(),
        meta: None,
    }
}

/// Plays `state` for `ticks`, issuing `orders` on the first tick, and
/// returns the ledgers.
fn play(mut state: State, ticks: u64, orders: Vec<PlayerCommand>) -> (State, Vec<SeatLedger>) {
    let mut ledger = ImpactLedger::new(&state);
    let mut orders = Some(orders);
    for _ in 0..ticks {
        let report = state.tick(&orders.take().unwrap_or_default());
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(&state);
    (state, ledgers)
}

fn unit_at(state: &State, x: i32, y: i32) -> UnitId {
    state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(x, y))
        .unwrap()
        .id
}

fn building(state: &State, player: u8, kind: BuildingKind) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(player) && building.kind == kind)
        .unwrap()
        .id
}

fn order(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn unit(ledgers: &[SeatLedger], seat: usize, kind: &str) -> KindLedger {
    ledgers[seat].units.get(kind).cloned().unwrap_or_default()
}

fn built(ledgers: &[SeatLedger], seat: usize, kind: &str) -> KindLedger {
    ledgers[seat]
        .buildings
        .get(kind)
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_kill_is_credited_to_its_shooter_at_the_victims_worth() {
    let scenario = field(
        0,
        &[
            (0, UnitKind::Sentinel, 14, 6),
            (1, UnitKind::Harvester, 16, 6),
        ],
        &[],
    );
    let (_, ledgers) = play(scenario.build().unwrap(), 600, Vec::new());
    let sentinel = unit(&ledgers, 0, "sentinel");
    let harvester = unit(&ledgers, 1, "harvester");
    let cost = u64::from(UnitKind::Harvester.stats().cost);
    assert_eq!(harvester.taken, cost, "{harvester:?}");
    assert_eq!(harvester.deaths, 1);
    assert_eq!(sentinel.dealt.workers, cost, "{sentinel:?}");
    assert_eq!(sentinel.dealt.total(), cost);
    assert_eq!(
        sentinel.dealt.field + sentinel.dealt.home + sentinel.dealt.away,
        cost
    );
    assert_eq!(ledgers[1].unattributed, 0);
}

#[test]
fn shooters_that_hit_one_victim_split_its_loss() {
    let scenario = field(
        0,
        &[
            (0, UnitKind::Sentinel, 14, 5),
            (0, UnitKind::Warden, 14, 7),
            (1, UnitKind::Sentinel, 16, 6),
        ],
        &[],
    );
    let (_, ledgers) = play(scenario.build().unwrap(), 600, Vec::new());
    let (sentinel, warden) = (unit(&ledgers, 0, "sentinel"), unit(&ledgers, 0, "warden"));
    let victim = unit(&ledgers, 1, "sentinel");
    assert_eq!(victim.deaths, 1);
    assert!(sentinel.dealt.army > 0 && warden.dealt.army > 0);
    assert!(
        (sentinel.dealt.army + warden.dealt.army).abs_diff(victim.taken) <= 1,
        "{sentinel:?} {warden:?} {victim:?}"
    );
}

#[test]
fn a_shell_is_credited_to_its_gun_and_to_the_spotter_that_saw_its_target() {
    let scenario = field(
        0,
        &[(0, UnitKind::Bombard, 8, 6), (0, UnitKind::Kestrel, 15, 6)],
        &[(1, BuildingKind::Barricade, 16, 6)],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let bombard = unit_at(&state, 8, 6);
    let barricade = building(&state, 1, BuildingKind::Barricade);
    assert!(
        UnitKind::Bombard.stats().vision < 8,
        "premise: beyond its sight"
    );
    let (_, ledgers) = play(
        state,
        1_200,
        vec![order(
            0,
            Command::Attack {
                units: vec![bombard],
                target: AttackTarget::Building(barricade),
                queue: false,
            },
        )],
    );
    let gun = unit(&ledgers, 0, "bombard");
    assert!(gun.dealt.buildings > 0, "{gun:?}");
    assert!(unit(&ledgers, 0, "kestrel").enabled > 0);
    assert_eq!(gun.enabled, 0);
}

#[test]
fn a_charge_is_credited_with_what_its_blast_does() {
    let scenario = field(
        0,
        &[(0, UnitKind::Sentinel, 12, 6)],
        &[(1, BuildingKind::ScuttleCharge, 16, 6)],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let sentinel = unit_at(&state, 12, 6);
    let (_, ledgers) = play(
        state,
        400,
        vec![order(
            0,
            Command::Run {
                units: vec![sentinel],
                goal: TilePos::new(20, 6),
                queue: false,
            },
        )],
    );
    let charge = built(&ledgers, 1, "scuttle charge");
    assert!(charge.dealt.army > 0, "{charge:?}");
    assert_eq!(charge.dealt.army, unit(&ledgers, 0, "sentinel").taken);
    assert_eq!(
        (charge.taken, ledgers[1].unattributed),
        (0, 0),
        "a charge spending itself is no one's damage"
    );
}

#[test]
fn a_sapper_is_credited_with_its_blast() {
    let scenario = field(
        0,
        &[(0, UnitKind::Sapper, 14, 6)],
        &[(1, BuildingKind::Barricade, 16, 6)],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let sapper = unit_at(&state, 14, 6);
    let barricade = building(&state, 1, BuildingKind::Barricade);
    let (_, ledgers) = play(
        state,
        400,
        vec![order(
            0,
            Command::Attack {
                units: vec![sapper],
                target: AttackTarget::Building(barricade),
                queue: false,
            },
        )],
    );
    let sapper = unit(&ledgers, 0, "sapper");
    assert!(sapper.dealt.buildings > 0, "{sapper:?}");
    assert_eq!(sapper.deaths, 1, "it blows itself up");
}

#[test]
fn direct_fire_splash_is_credited_to_its_shooter() {
    let scenario = field(
        0,
        &[
            (0, UnitKind::Breaker, 14, 6),
            (1, UnitKind::Harvester, 16, 6),
            (1, UnitKind::Harvester, 16, 7),
        ],
        &[],
    );
    let (_, ledgers) = play(scenario.build().unwrap(), 600, Vec::new());
    let breaker = unit(&ledgers, 0, "breaker");
    let harvesters = unit(&ledgers, 1, "harvester");
    assert_eq!(harvesters.deaths, 2, "{harvesters:?}");
    assert_eq!(ledgers[1].unattributed, 0, "splash lands on the shooter");
    assert_eq!(breaker.dealt.total(), harvesters.taken, "{breaker:?}");
}

#[test]
fn repair_is_credited_to_whoever_supplied_it() {
    let scenario = field(
        500,
        &[
            (0, UnitKind::Sentinel, 7, 6),
            (0, UnitKind::Sentinel, 14, 9),
            (0, UnitKind::Harvester, 15, 9),
        ],
        &[(0, BuildingKind::RepairBay, 5, 6)],
    );
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    for unit in value["units"].as_array_mut().unwrap() {
        if unit["kind"] == "sentinel" {
            unit["hp"] = serde_json::json!(20);
        }
    }
    let mut state: State = serde_json::from_value(value).unwrap();
    state.tick(&[]);
    let (welder, patient) = (unit_at(&state, 15, 9), unit_at(&state, 14, 9));
    let (_, ledgers) = play(
        state,
        1_200,
        vec![order(
            0,
            Command::RepairUnit {
                units: vec![welder],
                target: patient,
                queue: false,
            },
        )],
    );
    assert!(built(&ledgers, 0, "repair bay").repaired > 0);
    assert!(unit(&ledgers, 0, "harvester").repaired > 0);
}

#[test]
fn an_array_is_credited_with_a_hidden_charge_it_revealed() {
    let scenario = field(
        0,
        &[(0, UnitKind::Lancer, 12, 6)],
        &[
            (0, BuildingKind::Array, 12, 8),
            (1, BuildingKind::ScuttleCharge, 16, 6),
        ],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let lancer = unit_at(&state, 12, 6);
    let charge = building(&state, 1, BuildingKind::ScuttleCharge);
    let (_, ledgers) = play(
        state,
        600,
        vec![order(
            0,
            Command::Attack {
                units: vec![lancer],
                target: AttackTarget::Building(charge),
                queue: false,
            },
        )],
    );
    assert_eq!(built(&ledgers, 1, "scuttle charge").deaths, 1);
    assert_eq!(
        built(&ledgers, 0, "array").enabled,
        u64::from(BuildingKind::ScuttleCharge.invested_cost(0))
    );
}

#[test]
fn passive_income_matches_the_bank() {
    // A harvester keeps the seat off the recovery trickle, which the
    // ledger cannot credit.
    let scenario = field(
        0,
        &[(0, UnitKind::Harvester, 4, 8)],
        &[
            (0, BuildingKind::Reclaimer, 6, 6),
            (0, BuildingKind::Reclaimer, 8, 6),
        ],
    );
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    let reclaimer = value["buildings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|building| building["kind"] == "reclaimer")
        .unwrap();
    value["buildings"][reclaimer]["tier"] = serde_json::json!(1);
    let state: State = serde_json::from_value(value).unwrap();
    let before = state.players()[0].scrap;
    let (state, ledgers) = play(state, 3_000, Vec::new());
    let passive: u64 = ledgers[0]
        .buildings
        .values()
        .map(|totals| totals.passive)
        .sum();
    assert_eq!(passive, u64::from(state.players()[0].scrap - before));
    assert!(built(&ledgers, 0, "refinery").passive > built(&ledgers, 0, "reclaimer").passive);
    assert!(built(&ledgers, 0, "foundry").passive > 0, "the drip");
}

#[test]
fn spending_is_recorded_by_category_and_phase() {
    let scenario = field(
        1_000,
        &[(0, UnitKind::Harvester, 6, 8)],
        &[
            (0, BuildingKind::Fabricator, 8, 4),
            (0, BuildingKind::Reclaimer, 12, 4),
        ],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let foundry = building(&state, 0, BuildingKind::Foundry);
    let reclaimer = building(&state, 0, BuildingKind::Reclaimer);
    let harvester = unit_at(&state, 6, 8);
    let (_, ledgers) = play(
        state,
        1_200,
        vec![
            order(
                0,
                Command::Train {
                    building: foundry,
                    kind: UnitKind::Sentinel,
                },
            ),
            order(
                0,
                Command::Build {
                    units: vec![harvester],
                    kind: BuildingKind::Turret,
                    anchor: TilePos::new(6, 10),
                    queue: false,
                    defer: false,
                },
            ),
            order(
                0,
                Command::UpgradeBuilding {
                    building: reclaimer,
                },
            ),
        ],
    );
    let spend = &ledgers[0].spend;
    assert_eq!(
        spend["army:sentinel"][0],
        u64::from(UnitKind::Sentinel.stats().cost)
    );
    assert_eq!(
        spend["defense:turret"][0],
        u64::from(BuildingKind::Turret.invested_cost(0))
    );
    assert_eq!(
        spend["upgrade:refinery"][0],
        u64::from(BuildingKind::Reclaimer.invested_cost(1))
            - u64::from(BuildingKind::Reclaimer.invested_cost(0))
    );
    let refinery = built(&ledgers, 0, "refinery");
    assert_eq!(refinery.built + refinery.starting, 1);
    assert_eq!(
        (refinery.taken, ledgers[0].unattributed),
        (0, 0),
        "an upgrade rebuilding the building is no one's damage"
    );
    assert!(built(&ledgers, 0, "foundry").produced > 0);
}

#[test]
fn net_worth_is_sampled_from_bodies_and_the_bank() {
    let scenario = field(250, &[(0, UnitKind::Sentinel, 10, 6)], &[]);
    let (state, ledgers) = play(scenario.build().unwrap(), 2 * WORTH_PERIOD, Vec::new());
    assert_eq!(ledgers[0].worth.len(), 2);
    let expected: u64 = snapshot(&state)
        .iter()
        .filter(|body| body.owner == 0)
        .map(|body| body.value * u64::from(body.hp) / u64::from(body.max_hp.max(1)))
        .sum::<u64>()
        + u64::from(state.players()[0].scrap);
    assert_eq!(ledgers[0].worth[1], expected);
    assert_eq!(worth_at(&ledgers[0], 1_500), Some(ledgers[0].worth[0]));
    assert_eq!(worth_at(&ledgers[0], 9_000), Some(ledgers[0].worth[1]));
    assert_eq!(worth_at(&ledgers[0], 500), None);
}

#[test]
fn worth_shares_average_each_pair_before_pooling() {
    let mut pairs = PairShares::new();
    pairs.insert(
        "a".into(),
        vec![[Some(600), None, None, None], [Some(400), None, None, None]],
    );
    pairs.insert("b".into(), vec![[Some(700), None, None, None]]);
    let shares = worth_shares(&pairs);
    assert_eq!(shares.len(), 1, "only ticks some pair reached");
    assert_eq!(shares[0].pairs, 2);
    assert!((shares[0].mean - 0.6).abs() < 1e-9);
    assert!(shares[0].interval.is_some());
    let ledgers = [
        SeatLedger {
            worth: vec![1, 1, 1, 1, 1, 300],
            ..SeatLedger::default()
        },
        SeatLedger {
            worth: vec![1, 1, 1, 1, 1, 100],
            ..SeatLedger::default()
        },
    ];
    let refs: Vec<Option<&SeatLedger>> = ledgers.iter().map(Some).collect();
    assert_eq!(team_shares(&refs, &[0, 1], 0).unwrap()[0], Some(750));
    assert_eq!(team_shares(&[Some(&ledgers[0]), None], &[0, 1], 0), None);
}

/// `scenario` built, with `edit` applied to its JSON form.
fn staged(scenario: &Scenario, edit: impl FnOnce(&mut serde_json::Value)) -> State {
    let mut value = serde_json::to_value(scenario.build().unwrap()).unwrap();
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_shell_that_lands_after_its_gun_is_gone_is_still_its_guns() {
    let scenario = field(
        0,
        &[(0, UnitKind::Bombard, 8, 6), (0, UnitKind::Kestrel, 15, 6)],
        &[(1, BuildingKind::Barricade, 16, 6)],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let bombard = unit_at(&state, 8, 6);
    let barricade = building(&state, 1, BuildingKind::Barricade);
    let mut ledger = ImpactLedger::new(&state);
    let mut orders = vec![order(
        0,
        Command::Attack {
            units: vec![bombard],
            target: AttackTarget::Building(barricade),
            queue: false,
        },
    )];
    let mut fired = false;
    for _ in 0..600 {
        let report = state.tick(&std::mem::take(&mut orders));
        ledger.observe(&state, &report);
        if report
            .events
            .iter()
            .any(|event| matches!(event, Event::ShellLaunched { .. }))
        {
            fired = true;
            break;
        }
    }
    assert!(fired, "premise: the gun fires");
    let mut value = serde_json::to_value(&state).unwrap();
    value["units"]
        .as_array_mut()
        .unwrap()
        .retain(|unit| unit["kind"] != "bombard");
    let mut state: State = serde_json::from_value(value).unwrap();
    for _ in 0..200 {
        let report = state.tick(&[]);
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(&state);
    assert!(unit(&ledgers, 0, "bombard").dealt.buildings > 0);
    assert_eq!(ledgers[1].unattributed, 0);
}

#[test]
fn riders_count_toward_worth_and_die_to_whoever_downs_their_carrier() {
    let scenario = field(
        0,
        &[
            (0, UnitKind::Skyhook, 14, 6),
            (0, UnitKind::Sentinel, 4, 10),
        ],
        &[(1, BuildingKind::FlakTurret, 16, 6)],
    );
    let state = staged(&scenario, |value| {
        let units = value["units"].as_array_mut().unwrap();
        let rider = units
            .iter()
            .position(|unit| unit["kind"] == "sentinel")
            .unwrap();
        let rider = units.remove(rider);
        let skyhook = units
            .iter_mut()
            .find(|unit| unit["kind"] == "skyhook")
            .unwrap();
        skyhook["cargo"] = serde_json::json!([rider]);
    });
    let rider_value = u64::from(UnitKind::Sentinel.stats().cost);
    let ledger = ImpactLedger::new(&state);
    assert!(
        worth(&state, &ledger.bodies, 0) >= rider_value + u64::from(UnitKind::Skyhook.stats().cost),
        "the rider counts while aboard"
    );
    let (_, ledgers) = play(state, 1_200, Vec::new());
    let rider = unit(&ledgers, 0, "sentinel");
    assert_eq!((rider.deaths, rider.taken), (1, rider_value), "{rider:?}");
    assert_eq!(ledgers[0].unattributed, 0);
    assert!(built(&ledgers, 1, "flak turret").dealt.army >= rider_value);
}

#[test]
fn a_crucible_is_credited_with_the_wrecks_it_smelts() {
    let scenario = field(
        0,
        &[
            (0, UnitKind::Harvester, 4, 8),
            (0, UnitKind::Lancer, 10, 6),
            (0, UnitKind::Lancer, 10, 7),
            (1, UnitKind::Sentinel, 13, 6),
        ],
        &[(0, BuildingKind::Crucible, 9, 9)],
    );
    let state = scenario.build().unwrap();
    let before = state.players()[0].scrap;
    let (state, ledgers) = play(state, 1_200, Vec::new());
    let crucible = built(&ledgers, 0, "crucible");
    assert!(crucible.passive > 0, "{crucible:?}");
    let passive: u64 = ledgers[0]
        .buildings
        .values()
        .map(|totals| totals.passive)
        .sum();
    assert_eq!(passive, u64::from(state.players()[0].scrap - before));
}

#[test]
fn worth_runs_from_the_start_of_the_recording_to_its_end() {
    let scenario = field(250, &[(0, UnitKind::Sentinel, 10, 6)], &[]);
    let mut state = scenario.build().unwrap();
    for _ in 0..500 {
        state.tick(&[]);
    }
    let mut ledger = ImpactLedger::new(&state);
    for _ in 0..1_500 {
        let report = state.tick(&[]);
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(&state);
    let seat = &ledgers[0];
    assert_eq!((seat.start, seat.end, seat.worth.len()), (500, 2_000, 1));
    assert_eq!(worth_at(seat, 1_499), None, "before the first sample");
    assert_eq!(worth_at(seat, 1_500), Some(seat.worth[0]));
    let expected: u64 = snapshot(&state)
        .iter()
        .filter(|body| body.owner == 0)
        .map(|body| body.value * u64::from(body.hp) / u64::from(body.max_hp.max(1)))
        .sum::<u64>()
        + u64::from(state.players()[0].scrap);
    assert_eq!(seat.final_worth, expected);
    assert_eq!(
        worth_at(seat, 24_000),
        Some(expected),
        "a finished match keeps its end"
    );
    let sentinel = unit(&ledgers, 0, "sentinel");
    assert_eq!(sentinel.starting, 1);
}

#[test]
fn construction_does_not_hide_fire_on_a_site() {
    let scenario = field(
        500,
        &[
            (0, UnitKind::Harvester, 10, 8),
            (1, UnitKind::Sentinel, 16, 6),
        ],
        &[],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let harvester = unit_at(&state, 10, 8);
    let sentinel = unit_at(&state, 16, 6);
    let mut ledger = ImpactLedger::new(&state);
    let report = state.tick(&[order(
        0,
        Command::Build {
            units: vec![harvester],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(13, 6),
            queue: false,
            defer: false,
        },
    )]);
    ledger.observe(&state, &report);
    let site = building(&state, 0, BuildingKind::Turret);
    let mut orders = vec![order(
        1,
        Command::Attack {
            units: vec![sentinel],
            target: AttackTarget::Building(site),
            queue: false,
        },
    )];
    let mut hits = 0_u64;
    for _ in 0..400 {
        let report = state.tick(&std::mem::take(&mut orders));
        hits += report
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::AttackHit { target: Some(Target::Building(id)), .. } if *id == site
                )
            })
            .count() as u64;
        ledger.observe(&state, &report);
    }
    let ledgers = ledger.finish(&state);
    let turret = BuildingKind::Turret.tier_stats(0);
    let damage = u64::from(UnitKind::Sentinel.stats().weapons[0].damage);
    let expected =
        hits * damage * u64::from(BuildingKind::Turret.invested_cost(0)) / u64::from(turret.max_hp);
    let dealt = unit(&ledgers, 1, "sentinel").dealt.buildings;
    assert!(hits > 0, "premise: the site is under fire");
    assert!(
        dealt.abs_diff(expected) <= 1,
        "{dealt} dealt, {expected} expected from {hits} hits"
    );
}

#[test]
fn salvage_is_no_ones_damage() {
    let scenario = field(
        0,
        &[(0, UnitKind::Harvester, 9, 6)],
        &[(0, BuildingKind::Turret, 10, 6)],
    );
    let mut state = scenario.build().unwrap();
    state.tick(&[]);
    let harvester = unit_at(&state, 9, 6);
    let turret = building(&state, 0, BuildingKind::Turret);
    let (_, ledgers) = play(
        state,
        1_200,
        vec![order(
            0,
            Command::Salvage {
                units: vec![harvester],
                building: turret,
                queue: false,
            },
        )],
    );
    assert!(ledgers[0].refunds > 0, "premise: it was salvaged");
    assert_eq!(built(&ledgers, 0, "turret").taken, 0);
    assert_eq!(ledgers[0].unattributed, 0);
}

#[test]
fn a_crash_is_credited_to_the_aircraft_that_fell() {
    let mut units = vec![(0, UnitKind::Condor, 16, 6)];
    for y in 4..9 {
        for x in 14..19 {
            if (x, y) != (16, 6) {
                units.push((1, UnitKind::Sentinel, x, y));
            }
        }
    }
    let scenario = field(0, &units, &[(1, BuildingKind::FlakTurret, 20, 6)]);
    let (_, ledgers) = play(scenario.build().unwrap(), 1_200, Vec::new());
    let condor = unit(&ledgers, 0, "condor");
    assert_eq!(condor.deaths, 1, "premise: it is shot down");
    assert!(condor.dealt.army > 0, "{condor:?}");
    assert_eq!(ledgers[1].unattributed, 0);
}

#[test]
fn a_source_counts_once_per_victim_per_tick() {
    let scenario = field(
        0,
        &[(0, UnitKind::Sapper, 14, 6), (0, UnitKind::Sentinel, 14, 7)],
        &[(1, BuildingKind::Barricade, 16, 6)],
    );
    let state = scenario.build().unwrap();
    let mut ledger = ImpactLedger::new(&state);
    let (sapper, sentinel) = (unit_at(&state, 14, 6), unit_at(&state, 14, 7));
    let barricade = Key::Building(building(&state, 1, BuildingKind::Barricade));
    let source = |id| ledger.source(Key::Unit(id), None).unwrap();
    let mut sources = Sources::default();
    sources
        .aimed
        .insert(barricade, vec![source(sapper), source(sentinel)]);
    let at = find(&ledger.bodies, barricade).unwrap().tile;
    sources.splash.push((at, 2, source(sapper)));
    let before = ledger.bodies.clone();
    let target = *find(&before, barricade).unwrap();
    ledger.attribute(&before, &target, 100, false, &sources);
    let dealt = |id| ledger.records[&Key::Unit(id)].dealt.buildings;
    assert_eq!(
        dealt(sapper),
        dealt(sentinel),
        "the Sapper's blast is one hit"
    );
}

#[test]
fn crucibles_sharing_a_last_unit_of_wreck_do_not_both_smelt_it() {
    let scenario = field(
        0,
        &[],
        &[
            (0, BuildingKind::Crucible, 10, 6),
            (0, BuildingKind::Crucible, 14, 6),
        ],
    );
    let state = staged(&scenario, |value| {
        let width = value["map"]["grid"]["width"].as_i64().unwrap();
        let cell = usize::try_from(8 * width + 13).unwrap();
        value["map"]["grid"]["cells"][cell]["wreck"] = serde_json::json!(1);
    });
    assert_eq!(state.map().wreck_at(TilePos::new(13, 8)), 1);
    let smelting = smelters(&state);
    assert_eq!(smelting.len(), 1, "{smelting:?}");
}

#[test]
fn construction_gain_follows_the_simulations_ramp() {
    let scenario = field(0, &[], &[(0, BuildingKind::Turret, 10, 6)]);
    let state = scenario.build().unwrap();
    let mut site = *snapshot(&state)
        .iter()
        .find(|body| matches!(body.building, Some((BuildingKind::Turret, ..))))
        .unwrap();
    site.building = Some((BuildingKind::Turret, 0, false));
    let ramp = site.max_hp - site.max_hp / 5;
    let at = |progress: u32| Body { progress, ..site };
    assert_eq!(
        at(0).built_gain(&at(3)),
        None,
        "the first work sets the site up"
    );
    assert_eq!(
        at(10).built_gain(&at(12)),
        Some(ramp * 12 / site.build_ticks - ramp * 10 / site.build_ticks)
    );
    let built = Body {
        building: Some((BuildingKind::Turret, 0, true)),
        ..site
    };
    assert_eq!(built.built_gain(&built), Some(0));
}
