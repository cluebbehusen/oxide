//! The Crucible's smelter: a standing Crucible melts battlefield wrecks
//! within its ring into scrap, one unit per pulse — the immediate
//! utility that makes the tier-three climb a purchase instead of dead
//! spend. Fuel beyond the ring is not its to take.

use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, CRUCIBLE_SMELT_PERIOD};
use oxide_sim::{Faction, PlayerId, Scenario, State, UnitKind};

/// A walled yard: seat 0 owns a Crucible with a Turret beside it and a
/// second Turret far outside the smelter ring; seat 1's two doomed
/// harvesters stand at each Turret's feet. One parked own harvester
/// keeps the recovery machinery quiet so the bank moves only when the
/// smelter feeds it.
fn yard() -> Scenario {
    Scenario {
        name: "smelter-yard".into(),
        seed: 11,
        map: vec![
            "##############################".into(),
            "#1...........................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#..........................2.#".into(),
            "#............................#".into(),
            "##############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 2,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 12,
                y: 5,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 26,
                y: 5,
            },
        ],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: 8,
                y: 4,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 11,
                y: 4,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 25,
                y: 4,
            },
        ],
        meta: None,
    }
}

fn wreck_tiles(state: &State) -> Vec<(TilePos, u32)> {
    let mut tiles = Vec::new();
    for y in 0..10 {
        for x in 0..30 {
            let pos = TilePos::new(x, y);
            let amount = state.map().wreck_at(pos);
            if amount > 0 {
                tiles.push((pos, amount));
            }
        }
    }
    tiles
}

#[test]
fn the_smelter_melts_ring_fuel_and_leaves_the_far_field() {
    let mut state = yard().build().unwrap();
    // Let the turrets down both trespassers; their wrecks land at their
    // feet — one inside the smelter ring, one far outside it.
    for _ in 0..400 {
        state.tick(&[]);
        if wreck_tiles(&state).len() >= 2 {
            break;
        }
    }
    let field = wreck_tiles(&state);
    assert_eq!(field.len(), 2, "two kills leave two wreck tiles: {field:?}");
    let (near, near_amount) = field[0];
    let (far, far_amount) = field[1];
    assert!(near.x < 16 && far.x > 20, "one wreck per turret: {field:?}");

    let bank = state.player(PlayerId(0)).scrap;
    // Cross the next several pulse boundaries. Global wreck decay may
    // also step inside the window; the far wreck isolates its rate so
    // the smelter's exact draw stays provable.
    let start = state.current_tick();
    let until = (start / CRUCIBLE_SMELT_PERIOD + 5) * CRUCIBLE_SMELT_PERIOD;
    while state.current_tick() < until {
        state.tick(&[]);
    }
    let pulses = state.player(PlayerId(0)).scrap - bank;
    assert!(
        (4..=5).contains(&pulses),
        "one scrap per pulse crossed, got {pulses}"
    );
    let decay = far_amount - state.map().wreck_at(far);
    assert!(
        decay <= 1,
        "at most one decay step fits the window: {decay}"
    );
    assert_eq!(
        state.map().wreck_at(near),
        near_amount - pulses - decay,
        "the ring wreck fed exactly the scrap credited (plus global decay)"
    );
}
