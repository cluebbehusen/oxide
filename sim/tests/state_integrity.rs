//! The deserialization trust boundary, from both sides.
//!
//! One half is adversarial: a live state is serialized, one JSON pointer
//! is poked, and the forgery must be refused by name. Every row of
//! `State::validate_invariants` owes a fixture here, so a checklist row
//! that stops firing is a red test rather than a silent gap.
//!
//! The other half is the bring-up gate, and it is the load-bearing one:
//! a validator row TIGHTER than reality would refuse a state the sim
//! really produces, which is how a stricter validator turns into data
//! loss the day a snapshot save exists. So every shipped map plays out
//! bot-vs-bot with the checklist sampled along the way, a scripted run
//! exercises the verbs the bots rarely reach and is checked every single
//! tick, and a ticked state makes the full round trip through JSON.

use oxide_sim::bot::Brain;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, QUEUE_CAP};
use oxide_sim::{
    BuildingId, Command, Faction, PlayerCommand, PlayerId, Scenario, State, StateIntegrityError,
    UnitId, UnitKind,
};
use serde_json::{Value, json};

/// A two-seat arena with a standing Fabricator (a producer whose roster
/// spans both factions) and enough open ground for siege.
fn arena() -> Scenario {
    Scenario {
        name: "integrity-arena".into(),
        seed: 11,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#....s.............#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#................2.#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 900,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 900,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 4,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 12,
                y: 6,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 15,
                y: 6,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 16,
                y: 4,
            },
        ],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 4,
                y: 7,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 8,
                y: 7,
            },
        ],
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn run(state: &mut State, ticks: u32) {
    for _ in 0..ticks {
        state.tick(&[]);
    }
}

/// The base snapshot the forgeries are cut from: a few ticks in, so
/// orders, walks, and memories are real rather than freshly assembled.
fn snapshot() -> Value {
    let mut state = arena().build().unwrap();
    let (harvester, bombard) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Harvest {
                units: vec![harvester],
                node: chassis::grid::TilePos::new(5, 3),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::AttackMove {
                units: vec![bombard],
                goal: chassis::grid::TilePos::new(12, 6),
                queue: false,
            },
        ),
    ]);
    run(&mut state, 20);
    doc(&state)
}

fn doc(state: &State) -> Value {
    serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap()
}

/// The refusal a forged snapshot earns. Deserialization is the only path
/// into a `State`, so the validator speaks through serde's error.
fn refusal(doc: Value) -> String {
    serde_json::from_value::<State>(doc)
        .expect_err("a forged snapshot must never become a state")
        .to_string()
}

/// A well-formed shell, for fixtures that need one in the sky.
fn shell(shooter: Value, player: u32, impact_bits: i64) -> Value {
    json!({
        "shooter": shooter,
        "player": player,
        "launch": {"x": {"bits": 0}, "y": {"bits": 0}},
        "impact": {"x": {"bits": impact_bits}, "y": {"bits": 0}},
        "arrival": 40,
        "damage": 40,
        "targets": {"ground": true, "air": false},
        "splash": {"bits": 4294967296i64},
    })
}

/// A well-formed memory of an enemy building.
fn ghost(owner: u32, x: i32, y: i32) -> Value {
    json!({"kind": "turret", "owner": owner, "anchor": {"x": x, "y": y}, "hp": 350})
}

/// A well-formed recent allied impact record.
fn incident(x: i32, y: i32, expires_at: u64) -> Value {
    json!({"tile": {"x": x, "y": y}, "expires_at": expires_at})
}

#[test]
fn the_base_snapshot_is_accepted() {
    // Every forgery below is a single poke on this document; if the
    // document itself were refused, the fixtures would prove nothing.
    let base = snapshot();
    let restored: State = serde_json::from_value(base.clone()).expect("a real state round-trips");
    assert_eq!(doc(&restored), base, "the round trip is lossless");
}

#[test]
fn well_formed_additions_are_accepted() {
    // The hand-written shell, ghost, and contact shapes the forgeries
    // mutate must themselves be legal — otherwise a fixture could be
    // passing for the wrong reason.
    let mut base = snapshot();
    base["tick"] = json!(100);
    base["shells"].as_array_mut().unwrap().push(shell(
        json!({"kind": "unit", "id": 1}),
        0,
        4294967296,
    ));
    base["vision"][0]["ghosts"]
        .as_array_mut()
        .unwrap()
        .push(ghost(1, 12, 5));
    base["vision"][0]["contacts"] = json!([{"x": 3, "y": 3}, {"x": 1, "y": 4}]);
    base["vision"][0]["salvage_incidents"] = json!([incident(4, 3, 100), incident(2, 4, 100)]);
    serde_json::from_value::<State>(base).expect("legal additions stay legal");
}

#[test]
fn a_decided_state_may_age_past_an_incident_expiry() {
    let mut base = snapshot();
    base["tick"] = json!(100);
    base["result"] = json!({"outcome": "victory", "team": 0});
    base["vision"][0]["salvage_incidents"] = json!([incident(3, 3, 99)]);
    serde_json::from_value::<State>(base)
        .expect("decided worlds stop refreshing vision while their clock may still advance");
}

/// Contiguous index per checklist row, in declaration order. The
/// exhaustive match is the forcing chain: a new `StateIntegrityError`
/// variant fails to compile here until it takes an index, and the
/// coverage assertion below stays red until a forgery earns its
/// message.
fn row_index(e: &StateIntegrityError) -> usize {
    use StateIntegrityError as E;
    match e {
        E::NoPlayers => 0,
        E::TooManyPlayers => 1,
        E::ForeignTeam(_) => 2,
        E::InvalidRecoveryLedger(_) => 3,
        E::UnknownVictoryTeam(_) => 4,
        E::MalformedMapGrid => 5,
        E::MapTooLarge { .. } => 6,
        E::VisionTableMismatch => 7,
        E::MalformedVisionGrid => 8,
        E::UnsortedUnits => 9,
        E::UnsortedBuildings => 10,
        E::StaleUnitCounter => 11,
        E::StaleBuildingCounter => 12,
        E::TickBeyondEnvelope => 13,
        E::EliminationBeyondEnvelope(_) => 14,
        E::IdCounterBeyondEnvelope => 15,
        E::ForeignUnitOwner(_) => 16,
        E::UnitHpOutOfRange(_) => 17,
        E::UnitProgressOutOfRange(_) => 18,
        E::UnitCooldownOutOfRange(_) => 19,
        E::OverlongUnitQueue(_) => 20,
        E::UnitOutsideEnvelope(_) => 21,
        E::HarvestSourceOutsideZone(_) => 22,
        E::UnmintedOrderTarget(_) => 23,
        E::ForeignBuildingOwner(_) => 24,
        E::BuildingHpOutOfRange(_) => 25,
        E::BuildingProgressOutOfRange(_) => 26,
        E::BuildingCooldownOutOfRange(_) => 27,
        E::UnmintedBuildingFocus(_) => 28,
        E::InvalidBuildingFocus(_) => 29,
        E::OverlongBuildingQueue(_) => 30,
        E::UnproducibleQueueEntry(_) => 31,
        E::BuildingOutsideEnvelope(_) => 32,
        E::IncoherentSalvageLedger(_) => 33,
        E::TierBeyondLadder(_) => 34,
        E::LiveBuildingMarkedSalvaged(_) => 35,
        E::CargoOnNonTransport(_) => 50,
        E::CargoBeyondCapacity(_) => 51,
        E::UncarriableCargo(_) => 52,
        E::CargoHpOutOfRange(_) => 53,
        E::CargoOwnerMismatch(_) => 54,
        E::CargoNotDormant(_) => 55,
        E::AliasedCargoId => 56,
        E::ForeignShellOwner(_) => 36,
        E::ShellOutsideEnvelope(_) => 37,
        E::UnmintedShellShooter(_) => 38,
        E::ForeignGhostOwner(_) => 39,
        E::FriendlyGhost(_) => 40,
        E::GhostOutsideEnvelope(_) => 41,
        E::UnsortedGhosts(_) => 42,
        E::ContactOutsideEnvelope(_) => 43,
        E::UnsortedContacts(_) => 44,
        E::OverlongSalvageIncidentMemory(_) => 45,
        E::SalvageIncidentOutsideEnvelope(_) => 46,
        E::ExpiredSalvageIncident(_) => 47,
        E::SalvageIncidentExpiryBeyondHorizon(_) => 48,
        E::UnsortedSalvageIncidents(_) => 49,
    }
}

const ROWS: usize = 57;

/// One rendered message per row, with the entity ids the forgeries
/// provoke (everything targets seat p0 and entity 0). A fixture's
/// expected fragment must be a substring of exactly its row's message,
/// which is what lets string-matched forgeries prove enum-level
/// coverage.
fn row_examples() -> Vec<StateIntegrityError> {
    use StateIntegrityError as E;
    vec![
        E::NoPlayers,
        E::TooManyPlayers,
        E::ForeignTeam(PlayerId(0)),
        E::InvalidRecoveryLedger(PlayerId(0)),
        E::UnknownVictoryTeam(0),
        E::MalformedMapGrid,
        E::MapTooLarge {
            width: 300,
            height: 1,
        },
        E::VisionTableMismatch,
        E::MalformedVisionGrid,
        E::UnsortedUnits,
        E::UnsortedBuildings,
        E::StaleUnitCounter,
        E::StaleBuildingCounter,
        E::TickBeyondEnvelope,
        E::EliminationBeyondEnvelope(PlayerId(0)),
        E::IdCounterBeyondEnvelope,
        E::ForeignUnitOwner(UnitId(0)),
        E::UnitHpOutOfRange(UnitId(0)),
        E::UnitProgressOutOfRange(UnitId(0)),
        E::UnitCooldownOutOfRange(UnitId(0)),
        E::OverlongUnitQueue(UnitId(0)),
        E::UnitOutsideEnvelope(UnitId(0)),
        E::HarvestSourceOutsideZone(UnitId(0)),
        E::UnmintedOrderTarget(UnitId(0)),
        E::ForeignBuildingOwner(BuildingId(0)),
        E::BuildingHpOutOfRange(BuildingId(0)),
        E::BuildingProgressOutOfRange(BuildingId(0)),
        E::BuildingCooldownOutOfRange(BuildingId(0)),
        E::UnmintedBuildingFocus(BuildingId(0)),
        E::InvalidBuildingFocus(BuildingId(0)),
        E::OverlongBuildingQueue(BuildingId(0)),
        E::UnproducibleQueueEntry(BuildingId(0)),
        E::BuildingOutsideEnvelope(BuildingId(0)),
        E::IncoherentSalvageLedger(BuildingId(0)),
        E::TierBeyondLadder(BuildingId(0)),
        E::LiveBuildingMarkedSalvaged(BuildingId(0)),
        E::ForeignShellOwner(0),
        E::ShellOutsideEnvelope(0),
        E::UnmintedShellShooter(0),
        E::ForeignGhostOwner(PlayerId(0)),
        E::FriendlyGhost(PlayerId(0)),
        E::GhostOutsideEnvelope(PlayerId(0)),
        E::UnsortedGhosts(PlayerId(0)),
        E::ContactOutsideEnvelope(PlayerId(0)),
        E::UnsortedContacts(PlayerId(0)),
        E::OverlongSalvageIncidentMemory(PlayerId(0)),
        E::SalvageIncidentOutsideEnvelope(PlayerId(0)),
        E::ExpiredSalvageIncident(PlayerId(0)),
        E::SalvageIncidentExpiryBeyondHorizon(PlayerId(0)),
        E::UnsortedSalvageIncidents(PlayerId(0)),
        E::CargoOnNonTransport(UnitId(0)),
        E::CargoBeyondCapacity(UnitId(0)),
        E::UncarriableCargo(UnitId(0)),
        E::CargoHpOutOfRange(UnitId(0)),
        E::CargoOwnerMismatch(UnitId(0)),
        E::CargoNotDormant(UnitId(0)),
        E::AliasedCargoId,
    ]
}

/// One forgery: what it is called, the single poke that makes it, and
/// the fragment of the refusal it must earn.
type Forgery = (&'static str, fn(&mut Value), &'static str);

/// A dormant, well-formed Sentinel rider cut from the enemy Sentinel's
/// serialized shape: fresh id below the counter, idle, owner seat 0.
fn well_formed_rider(d: &Value) -> Value {
    let mut rider = d["units"][2].clone();
    let next = d["next_unit_id"].as_u64().expect("counter serialized");
    rider["id"] = json!(next - 1);
    rider["player"] = json!(0);
    rider["hp"] = json!(10);
    rider["order"] = json!({"order": "idle"});
    rider.as_object_mut().expect("unit is a map").remove("path");
    rider
        .as_object_mut()
        .expect("unit is a map")
        .remove("leash");
    rider
        .as_object_mut()
        .expect("unit is a map")
        .remove("queue");
    rider
}

/// Rewrites units[0] (the working Harvester) into a plausible Skyhook
/// so cargo clauses past the transport gate can be probed one at a time.
fn make_transport(d: &mut Value) {
    d["units"][0]["kind"] = json!("skyhook");
    d["units"][0]["hp"] = json!(150);
    d["units"][0]["order"] = json!({"order": "idle"});
    d["units"][0]["carrying"] = json!(0);
    let unit = d["units"][0].as_object_mut().expect("unit is a map");
    unit.remove("path");
    unit.remove("leash");
    unit.remove("queue");
}

/// Every checklist row, one forgery each. The expectation is the
/// message fragment the row names its victim with, so a row that stops
/// firing (or starts blaming the wrong entity) reads as a failure here.
#[test]
fn every_checklist_row_refuses_its_forgery() {
    let fixtures: Vec<Forgery> = vec![
        (
            "an empty player table",
            |d| d["players"] = json!([]),
            "no players",
        ),
        (
            "more seats than a player id can address",
            |d| {
                let seat = d["players"][0].clone();
                d["players"] = json!(vec![seat; 257]);
            },
            "more players than a player id can address",
        ),
        (
            "a team index no seat carries",
            |d| d["players"][0]["team"] = json!(9),
            "player p0 sits on a team outside the table",
        ),
        (
            "an emergency entitlement larger than its captured target",
            |d| {
                d["players"][0]["recovery_ready"] = json!(false);
                d["players"][0]["recovery_target"] = json!(50);
                d["players"][0]["recovery_allowance"] = json!(51);
            },
            "player p0 carries an invalid recovery ledger",
        ),
        (
            "a victory for a team no seat carries",
            |d| d["result"] = json!({"outcome": "victory", "team": 9}),
            "victory names team 9, which no player carries",
        ),
        (
            "a truncated map grid",
            |d| {
                d["map"]["grid"]["cells"].as_array_mut().unwrap().pop();
            },
            "map grid dimensions disagree with its cells",
        ),
        (
            "a map wider than the supported maximum",
            |d| {
                let cell = d["map"]["grid"]["cells"][0].clone();
                d["map"]["grid"] = json!({"width": 300, "height": 1, "cells": vec![cell; 300]});
            },
            "the supported maximum is 256 per side",
        ),
        (
            "a vision table short a seat",
            |d| {
                d["vision"].as_array_mut().unwrap().pop();
            },
            "vision table does not match the player list",
        ),
        (
            "a truncated vision grid",
            |d| {
                d["vision"][0]["visible"]["cells"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            },
            "disagrees with the map dimensions",
        ),
        (
            "units out of id order",
            |d| d["units"][1]["id"] = d["units"][0]["id"].clone(),
            "units not strictly sorted by id",
        ),
        (
            "buildings out of id order",
            |d| d["buildings"][1]["id"] = d["buildings"][0]["id"].clone(),
            "buildings not strictly sorted by id",
        ),
        (
            "a unit counter behind a live unit",
            |d| d["next_unit_id"] = json!(1),
            "unit id counter behind a live unit",
        ),
        (
            "a building counter behind a live building",
            |d| d["next_building_id"] = json!(1),
            "building id counter behind a live building",
        ),
        (
            "a tick past the envelope",
            |d| d["tick"] = json!(u64::MAX),
            "tick beyond the sanity envelope",
        ),
        (
            "an elimination stamp past the envelope",
            |d| d["players"][0]["eliminated_at"] = json!(u64::MAX),
            "elimination stamp lies beyond the sanity envelope",
        ),
        (
            "an id counter past the envelope",
            |d| d["next_unit_id"] = json!(u32::MAX),
            "an id counter is beyond the sanity envelope",
        ),
        (
            "a unit owned off the table",
            |d| d["units"][0]["player"] = json!(9),
            "unit u0 is owned by a player outside the table",
        ),
        (
            "a unit healthier than its kind can be",
            |d| d["units"][0]["hp"] = json!(999_999),
            "unit u0 carries hit points its kind cannot hold",
        ),
        (
            "a unit corpse that never got swept",
            |d| d["units"][0]["hp"] = json!(0),
            "unit u0 carries hit points its kind cannot hold",
        ),
        (
            "a unit work meter past the ceiling",
            |d| d["units"][0]["progress"] = json!(u32::MAX),
            "unit u0 carries a work meter past the ceiling",
        ),
        (
            "a cooldown on a weapon the harvester does not carry",
            |d| d["units"][0]["cooldowns"] = json!([5, 0]),
            "unit u0 carries a cooldown no weapon of its kind sets",
        ),
        (
            "an order queue past the cap",
            |d| d["units"][0]["queue"] = json!(vec![json!({"order": "idle"}); 33]),
            "unit u0 queues more orders than the cap allows",
        ),
        (
            "a unit shoved to the far end of the coordinate space",
            |d| d["units"][0]["pos"]["x"] = json!({"bits": i64::MAX}),
            "unit u0 names a coordinate outside the envelope",
        ),
        (
            "a walk routed through nonsense",
            |d| {
                d["units"][0]["path"] = json!({"goal": {"x": 1, "y": 1}, "waypoints": [{"x": i32::MAX, "y": 0}], "next": 0});
            },
            "unit u0 names a coordinate outside the envelope",
        ),
        (
            "a harvest work-zone anchor at the far end of the coordinate space",
            |d| {
                d["units"][0]["order"]["anchor"] = json!({"x": i32::MAX, "y": 0});
            },
            "unit u0 names a coordinate outside the envelope",
        ),
        (
            "a harvest source outside its anchored work zone",
            |d| {
                d["units"][0]["order"]["node"] = json!({"x": 15, "y": 8});
            },
            "unit u0 names a harvest source outside its work zone",
        ),
        (
            "an order against an id the run never minted",
            |d| {
                d["units"][0]["order"] =
                    json!({"order": "attack", "target": {"kind": "unit", "id": 9_999}});
            },
            "unit u0 is ordered against an id the run never minted",
        ),
        (
            "a building owned off the table",
            |d| d["buildings"][0]["player"] = json!(9),
            "building b0 is owned by a player outside the table",
        ),
        // "An unfinished Foundry" left this checklist in 0.15: Foundries
        // are buildable expansions now, so a Foundry site is a legal,
        // reachable state. The unconstructible-site invariant remains in
        // the validator for any future scenario-only kind.
        (
            "a building healthier than its kind can be",
            |d| d["buildings"][0]["hp"] = json!(999_999),
            "building b0 carries hit points its kind cannot hold",
        ),
        (
            "a progress meter past the ceiling",
            |d| d["buildings"][0]["progress"] = json!(u32::MAX),
            "building b0 carries a progress meter past the ceiling",
        ),
        (
            "a cooldown on a Foundry, which carries no weapon",
            |d| d["buildings"][0]["cooldown"] = json!(7),
            "building b0 carries a cooldown its weapon never sets",
        ),
        (
            "a defense focus against an id this run never minted",
            |d| {
                d["buildings"][3]["focus"] = json!({"kind": "unit", "id": 9_999});
            },
            "building b3 focuses an id the run never minted",
        ),
        (
            "a civilian building carrying a defense focus",
            |d| {
                d["buildings"][2]["focus"] = json!({"kind": "unit", "id": 2});
            },
            "building b2 carries an invalid defense focus",
        ),
        (
            "a production queue past the cap",
            |d| d["buildings"][0]["queue"] = json!(vec!["harvester"; QUEUE_CAP + 1]),
            "building b0 queues more units than the cap allows",
        ),
        (
            "a Foundry queuing a unit only the Fabricator trains",
            |d| d["buildings"][0]["queue"] = json!(["lancer"]),
            "building b0 queues a unit it could never train",
        ),
        (
            "a Ferrous Fabricator queuing the Cupric roster",
            |d| d["buildings"][2]["queue"] = json!(["stinger"]),
            "building b2 queues a unit it could never train",
        ),
        (
            "an anchor at the far end of the coordinate space",
            |d| d["buildings"][0]["anchor"]["x"] = json!(i32::MAX),
            "building b0 names a coordinate outside the envelope",
        ),
        (
            "a rally point at the far end of the coordinate space",
            |d| d["buildings"][0]["rally"] = json!({"x": -i32::MAX, "y": 0}),
            "building b0 names a coordinate outside the envelope",
        ),
        (
            "salvage credited against a building nothing stripped",
            |d| d["buildings"][0]["salvage_credited"] = json!(50),
            "building b0 carries an incoherent salvage ledger",
        ),
        (
            "a tier past the kind's ladder",
            |d| d["buildings"][0]["tier"] = json!(9),
            "claims a tier its kind's ladder does not reach",
        ),
        (
            "a live building marked as already salvaged",
            |d| d["buildings"][0]["salvaged"] = json!(true),
            "building b0 is still live but marked salvaged",
        ),
        (
            "cargo aboard a machine with no sling",
            |d| {
                let rider = well_formed_rider(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "carries cargo without being a transport",
        ),
        (
            "a sling packed past its capacity",
            |d| {
                let rider = well_formed_rider(d);
                make_transport(d);
                d["units"][0]["cargo"] = json!(vec![rider; 5]);
            },
            "carries more cargo than its sling holds",
        ),
        (
            "a rider no sling can take",
            |d| {
                let mut rider = well_formed_rider(d);
                rider["kind"] = json!("kestrel");
                make_transport(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "carries a rider that can never be carried",
        ),
        (
            "a dead rider in the hold",
            |d| {
                let mut rider = well_formed_rider(d);
                rider["hp"] = json!(0);
                make_transport(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "carries a rider with impossible hp",
        ),
        (
            "another player's machine in the hold",
            |d| {
                let mut rider = well_formed_rider(d);
                rider["player"] = json!(1);
                make_transport(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "carries another player's machine",
        ),
        (
            "a rider still holding live orders",
            |d| {
                let mut rider = well_formed_rider(d);
                rider["order"] = json!({"order": "move", "goal": {"x": 3, "y": 3}});
                make_transport(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "carries a rider that is not dormant",
        ),
        (
            "a rider whose id walks the world too",
            |d| {
                let mut rider = well_formed_rider(d);
                rider["id"] = d["units"][1]["id"].clone();
                make_transport(d);
                d["units"][0]["cargo"] = json!([rider]);
            },
            "aliased between the world and a cargo hold",
        ),
        (
            "a shell fired by a seat off the table",
            |d| {
                d["shells"].as_array_mut().unwrap().push(shell(
                    json!({"kind": "unit", "id": 1}),
                    9,
                    4294967296,
                ));
            },
            "shell 0 is owned by a player outside the table",
        ),
        (
            "a shell aimed at the far end of the coordinate space",
            |d| {
                d["shells"].as_array_mut().unwrap().push(shell(
                    json!({"kind": "unit", "id": 1}),
                    0,
                    i64::MAX,
                ));
            },
            "shell 0 names a coordinate outside the envelope",
        ),
        (
            "a shell fired by an id the run never minted",
            |d| {
                d["shells"].as_array_mut().unwrap().push(shell(
                    json!({"kind": "building", "id": 9_999}),
                    0,
                    4294967296,
                ));
            },
            "shell 0 was fired by an id the run never minted",
        ),
        (
            "a memory of a building owned off the table",
            |d| d["vision"][0]["ghosts"] = json!([ghost(9, 12, 5)]),
            "player p0 remembers a building owned outside the table",
        ),
        (
            "a memory of one's own building",
            |d| d["vision"][0]["ghosts"] = json!([ghost(0, 12, 5)]),
            "player p0 remembers a building of their own team",
        ),
        (
            "a memory anchored at the far end of the coordinate space",
            |d| d["vision"][0]["ghosts"] = json!([ghost(1, i32::MAX, 5)]),
            "player p0 remembers a building outside the envelope",
        ),
        (
            "memories out of canonical order",
            |d| d["vision"][0]["ghosts"] = json!([ghost(1, 3, 9), ghost(1, 3, 2)]),
            "player p0 remembers buildings out of canonical order",
        ),
        (
            "a radar blip at the far end of the coordinate space",
            |d| d["vision"][0]["contacts"] = json!([{"x": i32::MIN, "y": 0}]),
            "player p0 holds a radar contact outside the envelope",
        ),
        (
            "radar blips out of canonical order",
            |d| d["vision"][0]["contacts"] = json!([{"x": 1, "y": 4}, {"x": 3, "y": 3}]),
            "player p0 holds radar contacts out of canonical order",
        ),
        (
            "more recent impact sites than the bounded memory permits",
            |d| {
                d["vision"][0]["salvage_incidents"] = json!(
                    (0..=oxide_sim::stats::HARVEST_INCIDENT_CAP)
                        .map(|x| incident(x as i32, 3, 100))
                        .collect::<Vec<_>>()
                );
            },
            "player p0 holds more salvage incidents than the cap allows",
        ),
        (
            "a recent impact site at the far end of the coordinate space",
            |d| {
                d["vision"][0]["salvage_incidents"] = json!([incident(i32::MAX, 3, 100)]);
            },
            "player p0 remembers a salvage incident outside the envelope",
        ),
        (
            "an impact site that expired before the current tick",
            |d| {
                d["tick"] = json!(100);
                d["vision"][0]["salvage_incidents"] = json!([incident(3, 3, 99)]);
            },
            "player p0 carries an expired salvage incident in an active match",
        ),
        (
            "a recent impact expiry beyond the state's memory horizon",
            |d| {
                d["tick"] = json!(100);
                d["vision"][0]["salvage_incidents"] = json!([incident(
                    3,
                    3,
                    101 + oxide_sim::stats::HARVEST_INCIDENT_MEMORY_TICKS
                )]);
            },
            "player p0 carries a salvage incident expiry beyond its memory horizon",
        ),
        (
            "recent impact sites out of canonical order",
            |d| {
                d["vision"][0]["salvage_incidents"] =
                    json!([incident(2, 4, 100), incident(4, 3, 100)]);
            },
            "player p0 holds salvage incidents out of canonical order",
        ),
    ];

    // Enum-level coverage: the examples must map onto their own indices
    // (a duplicated index in row_index reads as a hole here), and every
    // row's rendered message must be earned by at least one forgery's
    // fragment.
    let examples = row_examples();
    assert_eq!(examples.len(), ROWS, "one example per checklist row");
    for (i, e) in examples.iter().enumerate() {
        assert_eq!(
            row_index(e),
            i,
            "row_index and row_examples disagree at {i}"
        );
    }
    // Ids differ between fixtures (whichever entity the poke hits) and
    // examples (always entity 0), so the match ignores digits and
    // anchors on the words.
    let no_digits = |s: &str| s.replace(|c: char| c.is_ascii_digit(), "");
    let mut covered = [false; ROWS];
    for (label, _, expected) in &fixtures {
        let rows: Vec<usize> = examples
            .iter()
            .enumerate()
            .filter(|(_, e)| no_digits(&e.to_string()).contains(&no_digits(expected)))
            .map(|(i, _)| i)
            .collect();
        assert!(
            !rows.is_empty(),
            "{label}: fragment '{expected}' matches no checklist row's message"
        );
        for i in rows {
            covered[i] = true;
        }
    }
    if let Some(row) = covered.iter().position(|c| !c) {
        panic!(
            "checklist row {row} ({}) has no forgery fixture",
            examples[row]
        );
    }

    let base = snapshot();
    for (label, poke, expected) in fixtures {
        let mut forged = base.clone();
        poke(&mut forged);
        let message = refusal(forged);
        assert!(
            message.contains(expected),
            "{label}: expected a refusal naming '{expected}', got '{message}'"
        );
    }
}

#[test]
fn the_ghost_sort_key_carries_the_owner() {
    // Two hostile seats can leave memories under the same corner, which
    // is exactly why the canonical key is (y, x, owner) and not (y, x).
    let mut base = snapshot();
    base["players"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "Third", "faction": "ferrous", "team": 2, "scrap": 0}));
    let view = base["vision"][0].clone();
    base["vision"].as_array_mut().unwrap().push(view);
    base["vision"][0]["ghosts"] = json!([ghost(1, 6, 6), ghost(2, 6, 6)]);
    serde_json::from_value::<State>(base.clone()).expect("owner-ordered memories are canonical");
    base["vision"][0]["ghosts"] = json!([ghost(2, 6, 6), ghost(1, 6, 6)]);
    assert!(
        refusal(base).contains("out of canonical order"),
        "the owner is part of the key, so reversing it breaks the order"
    );
}

#[test]
fn a_ticked_state_survives_a_json_round_trip() {
    let mut state = arena().build().unwrap();
    run(&mut state, 300);
    let restored: State =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).expect("a real state loads");
    assert_eq!(restored.hash(), state.hash(), "the round trip is exact");
}

/// The bring-up gate, narrow half: a run that deliberately drives the
/// verbs a bot rarely reaches — construction, welding, stripping,
/// patrol, artillery in flight — checked on EVERY tick. A row tighter
/// than reality fails here on the tick it becomes wrong.
#[test]
fn a_full_verb_run_stays_valid_every_tick() {
    let mut state = arena().build().unwrap();
    let (harvester, bombard, sentinel) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let fabricator = BuildingId(state.buildings()[2].id.0);
    let tile = chassis::grid::TilePos::new;

    let script: Vec<(u32, Vec<PlayerCommand>)> = vec![
        (
            0,
            vec![
                cmd(
                    0,
                    Command::Harvest {
                        units: vec![harvester],
                        node: tile(5, 3),
                        queue: false,
                    },
                ),
                cmd(
                    0,
                    Command::AttackMove {
                        units: vec![bombard],
                        goal: tile(13, 6),
                        queue: false,
                    },
                ),
                cmd(
                    1,
                    Command::Patrol {
                        units: vec![sentinel],
                        waypoints: vec![tile(15, 3), tile(15, 7), tile(13, 6)],
                    },
                ),
                cmd(
                    0,
                    Command::Train {
                        building: fabricator,
                        kind: UnitKind::Lancer,
                    },
                ),
            ],
        ),
        (
            40,
            vec![cmd(
                0,
                Command::Build {
                    units: vec![harvester],
                    kind: BuildingKind::Turret,
                    anchor: tile(3, 4),
                    queue: false,
                    defer: false,
                },
            )],
        ),
        (
            120,
            vec![cmd(
                0,
                Command::SetRally {
                    building: fabricator,
                    rally: Some(tile(5, 3)),
                },
            )],
        ),
        (
            420,
            vec![cmd(
                0,
                Command::Salvage {
                    units: vec![harvester],
                    building: fabricator,
                    queue: false,
                },
            )],
        ),
        (
            520,
            vec![cmd(
                0,
                Command::Repair {
                    units: vec![harvester],
                    building: fabricator,
                    queue: false,
                },
            )],
        ),
    ];

    let (mut saw_shell, mut saw_site, mut saw_strip, mut saw_weld) = (false, false, false, false);
    let mut saw_haul = false;
    let mut last_hp = state.building(fabricator).map(|b| b.hp);
    for tick in 0..1_200u32 {
        let commands = script
            .iter()
            .find(|(at, _)| *at == tick)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        state.tick(&commands);
        state.validate_invariants().unwrap_or_else(|err| {
            panic!("tick {tick} produced a state the validator refuses: {err}")
        });
        saw_shell |= !state.shells().is_empty();
        saw_haul |= state.units().iter().any(|u| u.carrying > 0);
        saw_site |= state.buildings().iter().any(|b| !b.built);
        let hp = state.building(fabricator).map(|b| b.hp);
        saw_strip |= matches!((last_hp, hp), (Some(was), Some(now)) if now < was);
        saw_weld |= matches!((last_hp, hp), (Some(was), Some(now)) if now > was);
        last_hp = hp;
    }
    // The script really did reach the interesting shapes.
    assert!(saw_shell, "premise: artillery was airborne");
    assert!(saw_haul, "premise: scrap was hauled");
    assert!(saw_site, "premise: a site stood unfinished");
    assert!(saw_strip, "premise: the Fabricator was stripped");
    assert!(saw_weld, "premise: the Fabricator was welded back");
    assert!(
        state
            .building(fabricator)
            .is_some_and(|b| b.salvage_drained > 0 && b.salvage_credited > 0),
        "premise: the salvage ledger carries a real entry"
    );
}

/// The bring-up gate, broad half: every shipped map, every seat driven,
/// thousands of ticks, the checklist sampled along the way. Independent
/// deterministic sims, so the sweep fans across threads like the other
/// map sweeps.
#[test]
fn every_shipped_map_stays_valid_under_bot_play() {
    // Re-anchored on the all-Overseer sweep: its first construction
    // lands around tick 1,100-3,600 depending on the map (the deleted
    // 0.14 actors built earlier), and at 8,000 ticks every shipped map
    // measures both built-up (34/34) and remembered (34/34).
    const TICKS: u32 = 8_000;
    /// How often the checklist runs directly (cheap: entities, not tiles).
    const SAMPLE: u32 = 100;
    /// How often the state makes the full trip through the deserializer,
    /// which is where the checklist actually stands guard.
    const ROUND_TRIP: u32 = 500;

    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("scenarios dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    assert!(paths.len() >= 10, "the shipped roster is present");

    // A sweep that stopped producing memories, sites, or fresh machines
    // would still pass every row while proving nothing.
    let built_up = std::sync::atomic::AtomicUsize::new(0);
    let remembered = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for path in &paths {
            let (built_up, remembered) = (&built_up, &remembered);
            scope.spawn(move || {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                let scenario = Scenario::load(path).unwrap_or_else(|err| panic!("{name}: {err}"));
                let seed = scenario.seed;
                let mut state = scenario
                    .build()
                    .unwrap_or_else(|err| panic!("{name}: {err}"));
                let (scenario_units, scenario_buildings) =
                    (state.units().len(), state.buildings().len());
                // Every chair thinks: the widest spread of live orders,
                // construction, and battle the shipped maps can produce.
                // The Overseer is the one commander left standing while
                // bot seats await the retrained actor.
                let mut brains: Vec<Brain> = (0..state.players().len())
                    .map(|i| Brain::overseer(PlayerId(i as u8), seed))
                    .collect();
                let mut saw_ghost = false;
                for tick in 0..TICKS {
                    let commands: Vec<PlayerCommand> =
                        brains.iter_mut().flat_map(|b| b.act(&state)).collect();
                    state.tick(&commands);
                    if tick.is_multiple_of(SAMPLE) {
                        state.validate_invariants().unwrap_or_else(|err| {
                            panic!("{name}: tick {tick} is a state the validator refuses: {err}")
                        });
                    }
                    if tick.is_multiple_of(ROUND_TRIP) {
                        let restored: State =
                            serde_json::from_str(&serde_json::to_string(&state).unwrap())
                                .unwrap_or_else(|err| {
                                    panic!("{name}: tick {tick} is refused at the door: {err}")
                                });
                        assert_eq!(
                            restored.hash(),
                            state.hash(),
                            "{name}: tick {tick} round-trips exactly"
                        );
                    }
                    saw_ghost |= (0..state.players().len())
                        .any(|i| !state.vision(PlayerId(i as u8)).ghosts().is_empty());
                }
                let rich = state.units().len() > scenario_units
                    && state.buildings().len() > scenario_buildings;
                if rich {
                    built_up.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if saw_ghost {
                    remembered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    let (built_up, remembered) = (
        built_up.load(std::sync::atomic::Ordering::Relaxed),
        remembered.load(std::sync::atomic::Ordering::Relaxed),
    );
    eprintln!(
        "sweep tallies: built_up {built_up}, remembered {remembered}, maps {}",
        paths.len()
    );
    assert!(
        built_up * 3 >= paths.len(),
        "the sweep must reach built-up worlds, not idle openings ({built_up} of {})",
        paths.len()
    );
    assert!(
        remembered * 3 >= paths.len(),
        "the sweep must reach worlds carrying enemy memories ({remembered} of {})",
        paths.len()
    );
}

#[test]
fn a_dangling_order_target_is_not_a_forgery() {
    // The permissive reference rule, stated as a test: an order outliving
    // its victim by a tick is ordinary play, and refusing it would refuse
    // a state the sim produces every time something dies mid-chase.
    let mut base = snapshot();
    let live = base["units"][0]["id"].as_u64().unwrap() as u32;
    let minted = base["next_unit_id"].as_u64().unwrap() as u32;
    assert!(live < minted, "premise: the counter is ahead of the roster");
    base["units"][0]["order"] =
        json!({"order": "attack", "target": {"kind": "unit", "id": minted - 1}});
    let state: State =
        serde_json::from_value(base).expect("a minted-but-departed target is accepted");
    // And it stays a legal state as the brains discover the victim is gone.
    let mut state = state;
    run(&mut state, 5);
    state.validate_invariants().expect("still coherent");
    assert!(state.unit(UnitId(live)).is_some());
}
