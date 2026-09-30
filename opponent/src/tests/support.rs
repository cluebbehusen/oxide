use super::*;
use crate::defenses;
use crate::expansion;
use crate::frame::gap;
use crate::investments::Investment;
use crate::memory::Memory;
use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;

fn building(player: u8, kind: BuildingKind, x: i32, y: i32) -> BuildingSpec {
    BuildingSpec { player, kind, x, y }
}

/// `state` with `building` at `hp`.
fn damaged(state: &State, building: BuildingId, hp: u32) -> State {
    let mut value = serde_json::to_value(state).unwrap();
    let buildings = value["buildings"].as_array_mut().unwrap();
    let entry = buildings
        .iter_mut()
        .find(|entry| entry["id"] == building.0)
        .unwrap();
    entry["hp"] = hp.into();
    serde_json::from_value(value).unwrap()
}

/// `state` with each of `units` at `hp`.
fn all_wounded(state: &State, units: &[UnitId], hp: u32) -> State {
    units
        .iter()
        .fold(state.clone(), |state, unit| wounded(&state, *unit, hp))
}

fn repairs(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, BuildingId)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Repair {
                units, building, ..
            } => Some((units.clone(), *building)),
            _ => None,
        })
        .collect()
}

fn welds(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, UnitId)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::RepairUnit { units, target, .. } => Some((units.clone(), *target)),
            _ => None,
        })
        .collect()
}

/// West Sentinel spots near home, clear of the arena's scrap.
const SQUAD: [(i32, i32); 8] = [
    (2, 8),
    (3, 8),
    (4, 8),
    (5, 8),
    (2, 10),
    (3, 10),
    (4, 10),
    (5, 10),
];

/// The arena with West's Sentinels on `SQUAD`, and their ids.
fn squad(scrap: u32) -> (Scenario, impl Fn(&State) -> Vec<UnitId>) {
    let mut scenario = arena(scrap);
    for (x, y) in SQUAD {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    let ids = |state: &State| SQUAD.iter().map(|(x, y)| at(state, *x, *y)).collect();
    (scenario, ids)
}

#[test]
fn a_fabricator_turns_two_open_harvester_slots_into_an_excavator() {
    let trained = |fabricator: bool| {
        let mut scenario = arena(1_000);
        scenario.units.extend(standing_army(0));
        if fabricator {
            scenario
                .buildings
                .push(building(0, BuildingKind::Fabricator, 3, 1));
        }
        let state = scenario.build().unwrap();
        trains(&seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default()))
            .into_iter()
            .map(|(_, kind)| kind)
            .collect::<Vec<_>>()
    };
    let without = trained(false);
    assert!(without.contains(&UnitKind::Harvester), "{without:?}");
    let with = trained(true);
    assert!(with.contains(&UnitKind::Excavator), "{with:?}");
    assert!(
        !with.contains(&UnitKind::Harvester),
        "the Excavator fills both open slots: {with:?}"
    );
}

#[test]
fn workers_weld_a_damaged_building_unless_an_enemy_stands_near() {
    let welded = |raider: bool| {
        let mut scenario = arena(200);
        if raider {
            scenario.units.push(unit(1, UnitKind::Sentinel, 10, 6));
        }
        let state = scenario.build().unwrap();
        let foundry = foundries(&state, PlayerId(0))[0];
        let max = BuildingKind::Foundry.base_stats().max_hp;
        let state = damaged(&state, foundry, max / 2);
        let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
        (foundry, repairs(&commands))
    };
    let (foundry, calm) = welded(false);
    let [(welders, patient)] = &calm[..] else {
        panic!("{calm:?}");
    };
    assert_eq!(*patient, foundry);
    assert_eq!(welders.len(), 1, "one worker at a time");
    let (_, raided) = welded(true);
    assert!(raided.is_empty(), "{raided:?}");
}

#[test]
fn idle_workers_keep_off_a_node_in_known_danger() {
    let nodes = |raider: bool| {
        let mut scenario = arena(0);
        if raider {
            scenario.units.push(unit(1, UnitKind::Sentinel, 9, 10));
        }
        let state = scenario.build().unwrap();
        let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
        harvests(&commands)
            .into_iter()
            .map(|(node, _)| node)
            .collect::<Vec<_>>()
    };
    let guarded = TilePos::new(7, 9);
    assert!(
        nodes(false).contains(&guarded),
        "premise: {:?}",
        nodes(false)
    );
    let sent = nodes(true);
    assert!(!sent.is_empty() && !sent.contains(&guarded), "{sent:?}");
}

#[test]
fn a_worker_away_from_home_runs_from_an_armed_enemy() {
    let mut scenario = arena(0);
    scenario
        .units
        .extend([harvester(0, 15, 9), unit(1, UnitKind::Sentinel, 18, 10)]);
    let state = scenario.build().unwrap();
    let worker = at(&state, 15, 9);
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let (_, refuge) = runs(&commands)
        .into_iter()
        .find(|(units, _)| *units == [worker])
        .unwrap_or_else(|| panic!("{commands:?}"));
    assert_eq!(
        gap(TilePos::new(3, 5), (2, 2), refuge, (1, 1)),
        0,
        "beside the Foundry: {refuge:?}"
    );
}

#[test]
fn a_worker_away_from_home_runs_from_artillery_in_range() {
    let mut scenario = arena(0);
    scenario.units.extend([
        harvester(0, 15, 9),
        unit(1, UnitKind::Bombard, 15, 1),
        unit(0, UnitKind::Kestrel, 15, 3),
    ]);
    let state = scenario.build().unwrap();
    let worker = at(&state, 15, 9);
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(
        runs(&commands).iter().any(|(units, _)| *units == [worker]),
        "a Bombard eight tiles off still reaches it: {commands:?}"
    );
}

#[test]
fn a_wounded_army_brings_a_tender() {
    let tenders = |hp: u32| {
        let (mut scenario, ids) = squad(400);
        scenario
            .buildings
            .push(building(0, BuildingKind::Fabricator, 3, 1));
        let state = scenario.build().unwrap();
        let state = all_wounded(&state, &ids(&state), hp);
        trains(&seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default()))
            .into_iter()
            .filter(|(_, kind)| *kind == UnitKind::Tender)
            .count()
    };
    assert_eq!(tenders(UnitKind::Sentinel.stats().max_hp), 0);
    assert_eq!(tenders(1), 1);
}

#[test]
fn a_free_tender_welds_the_most_wounded_free_unit() {
    let mut scenario = arena(200);
    scenario.units.extend([
        unit(0, UnitKind::Tender, 5, 8),
        unit(0, UnitKind::Sentinel, 3, 10),
        unit(0, UnitKind::Sentinel, 4, 10),
    ]);
    let state = scenario.build().unwrap();
    let (tender, grazed, mauled) = (at(&state, 5, 8), at(&state, 3, 10), at(&state, 4, 10));
    let state = wounded(&wounded(&state, grazed, 40), mauled, 10);
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert_eq!(welds(&commands), [(vec![tender], mauled)]);
}

/// An attack regrouping at its rally with the arena's West squad (one of
/// them still walking in), `tended` when a Tender already belongs to it, and
/// the squad's ids.
fn regrouping(tended: bool) -> (State, Opponent, Vec<UnitId>, UnitId) {
    let (mut scenario, ids) = squad(0);
    scenario.units.push(unit(0, UnitKind::Tender, 6, 9));
    let mut state = scenario.build().unwrap();
    let squad = ids(&state);
    let tender = at(&state, 6, 9);
    let walker = squad[7];
    state.tick(&[run(0, vec![walker], 5, 11)]);
    while state.current_tick() < 12 {
        state.tick(&[]);
    }
    let mut members = squad.clone();
    if tended {
        members.push(tender);
    }
    members.sort_unstable();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "attack",
                "target": {"owner": 1, "building": "foundry", "anchor": {"x": 19, "y": 5}},
                "phase": "recover",
            },
            "since": 12,
            "units": members,
            "goal": {"x": 6, "y": 9},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    (state, opponent, squad, tender)
}

#[test]
fn a_regrouping_attack_takes_a_free_tender_along() {
    let (state, mut opponent, _, tender) = regrouping(false);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(
        runs(&commands).iter().any(|(units, _)| *units == [tender]),
        "{commands:?}"
    );
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let members = json["missions"]["list"][0]["units"].as_array().unwrap();
    assert!(members.contains(&tender.0.into()), "{members:?}");
}

#[test]
fn a_tender_welds_its_attack_while_the_army_regroups() {
    let (state, mut opponent, squad, tender) = regrouping(true);
    let hurt = squad[0];
    let state = wounded(&state, hurt, 30);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(welds(&commands), [(vec![tender], hurt)]);
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

/// Where West would put a Repair Bay in `state`, if anywhere.
fn bay(scenario: &Scenario, state: &State) -> Option<TilePos> {
    let observation = ObservationData::fog_honest(state, PlayerId(0));
    defenses::investments(
        &observation,
        &map(scenario),
        &Memory::default(),
        traits(),
        true,
        false,
    )
    .into_iter()
    .find_map(|(investment, _)| match investment {
        Investment::Defense {
            kind: BuildingKind::RepairBay,
            anchor,
        } => Some(anchor),
        _ => None,
    })
}

#[test]
fn a_repair_bay_goes_up_where_the_wounded_gather() {
    let (mut scenario, _) = squad(0);
    let hurt = |scenario: &Scenario, hp: u32| {
        let state = scenario.build().unwrap();
        let sentinels: Vec<UnitId> = state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
            .map(|unit| unit.id)
            .collect();
        all_wounded(&state, &sentinels, hp)
    };
    let healthy = hurt(&scenario, UnitKind::Sentinel.stats().max_hp);
    assert_eq!(bay(&scenario, &healthy), None, "nobody is hurt");
    let anchor = bay(&scenario, &hurt(&scenario, 1)).expect("a Repair Bay is wanted");
    let reached = SQUAD
        .iter()
        .filter(|(x, y)| gap(anchor, (2, 2), TilePos::new(*x, *y), (1, 1)) <= 3)
        .count();
    assert!(reached >= SQUAD.len() / 2, "{anchor:?} reaches {reached}");

    scenario
        .units
        .retain(|unit| gap(anchor, (2, 2), TilePos::new(unit.x, unit.y), (1, 1)) >= 0);
    scenario
        .buildings
        .push(building(0, BuildingKind::RepairBay, anchor.x, anchor.y));
    let second = bay(&scenario, &hurt(&scenario, 1));
    assert!(
        second.is_none_or(|second| gap(anchor, (2, 2), second, (2, 2)) > 3),
        "a second Bay only for wounded the first cannot reach: {second:?}"
    );
}

#[test]
fn an_extractor_frame_waits_while_an_armed_enemy_stands_near() {
    let mut scenario = arena(0);
    scenario.map[1].replace_range(6..7, "E");
    let frame = TilePos::new(6, 1);
    let wanted = |scenario: &Scenario, memory: &mut Memory| {
        let state = scenario.build().unwrap();
        let observation = ObservationData::fog_honest(&state, PlayerId(0));
        memory.observe(&observation);
        expansion::candidates(&observation, &map(scenario), memory, 50, 0)
            .iter()
            .any(|(investment, _)| *investment == Investment::Extractor(frame))
    };
    let mut memory = Memory::default();
    assert!(wanted(&scenario, &mut memory), "premise");
    let mut raided = scenario.clone();
    raided.units.push(unit(1, UnitKind::Sentinel, 8, 2));
    assert!(!wanted(&raided, &mut memory));
    assert!(
        wanted(&scenario, &mut memory),
        "once the enemy is out of sight the frame is wanted again"
    );
}
