use super::*;
use crate::defenses;
use crate::investments::{self, Investment};
use crate::memory::Memory;
use crate::placement::{self, Danger, Layout};
use chassis::fx::Fx;

/// The arena with each seat's workforce and standing army along its edge,
/// leaving the ground around each start clear.
fn saturated(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.units = workforce(0);
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

fn footprint(kind: BuildingKind, anchor: TilePos) -> impl Iterator<Item = TilePos> {
    let (width, height) = kind.base_stats().size;
    (0..height).flat_map(move |dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
}

/// Stands `kind` for `player` at `anchor`.
fn put(scenario: &mut Scenario, player: u8, kind: BuildingKind, anchor: TilePos) {
    scenario.buildings.push(BuildingSpec {
        player,
        kind,
        x: anchor.x,
        y: anchor.y,
    });
}

/// Takes the two-by-two spot at `anchor` with a Turret on a tile of it no
/// unit stands on.
fn take(scenario: &mut Scenario, anchor: TilePos) {
    let tile = footprint(BuildingKind::Fabricator, anchor)
        .find(|tile| {
            !scenario
                .units
                .iter()
                .any(|unit| TilePos::new(unit.x, unit.y) == *tile)
        })
        .expect("a free tile in the spot");
    put(scenario, 0, BuildingKind::Turret, tile);
}

/// The two-by-two buildings other than defenses among `commands`' builds.
fn packed(commands: &[PlayerCommand]) -> Vec<(BuildingKind, TilePos)> {
    builds(commands)
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
        .collect()
}

/// Takes every two-by-two spot within `gap` of West's start but `keep`.
fn crowd(scenario: &mut Scenario, gap: i32, keep: &[TilePos]) {
    let model = map(scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let near: Vec<TilePos> = model
        .spots(PlayerId(0), vec![start], BuildingKind::Fabricator)
        .take_while(|spot| crate::frame::gap(start, (2, 2), *spot, (2, 2)) <= gap)
        .collect();
    assert!(
        keep.iter().all(|spot| near.contains(spot)),
        "premise: {keep:?} are spots: {near:?}"
    );
    for spot in near {
        if !keep.contains(&spot) {
            take(scenario, spot);
        }
    }
}

#[test]
fn a_building_packs_beside_the_last_one_off_the_lanes() {
    let mut scenario = saturated(400);
    let model = map(&scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let first = TilePos::new(7, 6);
    let beside = [TilePos::new(9, 4), TilePos::new(9, 6)];
    put(&mut scenario, 0, BuildingKind::Crucible, first);
    crowd(&mut scenario, 4, &[first, beside[0], beside[1]]);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    for spot in beside {
        assert_eq!(
            placement::check(
                &observation,
                BuildingKind::Fabricator,
                spot,
                &[],
                Layout::Apart
            ),
            Err(placement::Refusal::Crowded),
            "premise: a building kept apart could not stand there"
        );
    }
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let laid = packed(&commands);
    assert!(
        !laid.is_empty(),
        "premise: a building is bought: {commands:?}"
    );
    for (kind, anchor) in laid {
        assert!(beside.contains(&anchor), "{kind:?} at {anchor:?}");
        assert_eq!(
            crate::frame::gap(first, (2, 2), anchor, kind.base_stats().size),
            0
        );
        assert!(
            footprint(kind, anchor).all(|tile| !model.lane_of(&[start], tile)),
            "{kind:?} at {anchor:?} stands on a lane"
        );
    }
}

/// West's start pocket opens east only through a corridor two tiles wide,
/// whose mouth is a slot of its layout.
const MOUTH: [&str; 12] = [
    "########################",
    "#........##............#",
    "#........##.....s......#",
    "#......s.##............#",
    "#........##............#",
    "#..1.....##........2...#",
    "#......................#",
    "#......................#",
    "#........##.....s......#",
    "#......s.##............#",
    "#........##............#",
    "########################",
];

#[test]
fn a_building_never_closes_the_way_out_of_the_base() {
    let mut scenario = saturated(400);
    scenario.map = MOUTH.map(str::to_owned).to_vec();
    for unit in &mut scenario.units {
        if (unit.x, unit.y) == (9, 10) {
            (unit.x, unit.y) = (4, 9);
        }
    }
    let model = map(&scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let mouth = TilePos::new(7, 6);
    let corridor = TilePos::new(9, 6);
    for spot in model
        .spots(PlayerId(0), vec![start], BuildingKind::Fabricator)
        .take_while(|spot| crate::frame::gap(start, (2, 2), *spot, (2, 2)) <= 2)
    {
        if spot != mouth {
            take(&mut scenario, spot);
        }
    }
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let first = investments::anchors(
        &model,
        &observation,
        Investment::Tech(BuildingKind::Fabricator),
        BuildingKind::Fabricator,
    )
    .find(|anchor| {
        placement::check(
            &observation,
            BuildingKind::Fabricator,
            *anchor,
            &[],
            Layout::Packed,
        )
        .is_ok()
    });
    assert_eq!(first, Some(mouth), "premise: the mouth is the first spot");
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let laid = packed(&commands);
    assert!(
        !laid.is_empty(),
        "premise: a building is bought: {commands:?}"
    );
    for (kind, anchor) in laid {
        assert!(
            ![mouth, corridor].contains(&anchor),
            "{kind:?} at {anchor:?} closes the corridor"
        );
    }
}

#[test]
fn another_foundry_takes_an_empty_block_of_its_own() {
    let mut scenario = saturated(400);
    put(
        &mut scenario,
        0,
        BuildingKind::Fabricator,
        TilePos::new(9, 4),
    );
    let model = map(&scenario);
    let start = model.start(PlayerId(0)).unwrap();
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let anchor = investments::anchors(
        &model,
        &observation,
        Investment::Capacity(BuildingKind::Foundry),
        BuildingKind::Foundry,
    )
    .find(|anchor| {
        matches!(
            placement::check(
                &observation,
                BuildingKind::Foundry,
                *anchor,
                &[],
                investments::layout(
                    Investment::Capacity(BuildingKind::Foundry),
                    BuildingKind::Foundry,
                ),
            ),
            Ok(_) | Err(placement::Refusal::Unexplored)
        )
    })
    .expect("room for another Foundry");
    let block: Vec<TilePos> = footprint(BuildingKind::Foundry, anchor)
        .chain(crate::frame::ring(anchor, (2, 2)))
        .collect();
    assert!(
        block.iter().all(|tile| !model.lane_of(&[start], *tile)),
        "{anchor:?}: a Foundry and its ring fill one block"
    );
    assert!(
        crate::frame::ring(anchor, (2, 2)).all(|tile| {
            observation.my_buildings.iter().all(|building| {
                !footprint(building.kind, building.anchor).any(|other| other == tile)
            })
        }),
        "{anchor:?}: the block is empty"
    );
}

/// The buildings West buys first on `scenario` once `threat` is staged.
fn under_in(
    mut scenario: Scenario,
    threat: impl Fn(&mut Scenario),
) -> Vec<(BuildingKind, TilePos)> {
    threat(&mut scenario);
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    packed(&commands)
}

fn under(threat: impl Fn(&mut Scenario)) -> Vec<(BuildingKind, TilePos)> {
    under_in(saturated(400), threat)
}

/// Whether a `kind` at `anchor` lies within `range` tiles of `centre`, in
/// doubled coordinates, measured to the footprint's nearest point.
fn within(kind: BuildingKind, anchor: TilePos, centre: (i64, i64), range: Fx) -> bool {
    let (width, height) = kind.base_stats().size;
    let axis = |v: i64, low: i32, len: i32| {
        let (low, high) = (2 * i64::from(low), 2 * i64::from(low + len));
        (low - v).max(v - high).max(0)
    };
    let (dx, dy) = (
        axis(centre.0, anchor.x, width),
        axis(centre.1, anchor.y, height),
    );
    let reach = (range + range).to_num::<i64>();
    dx * dx + dy * dy <= reach * reach
}

#[test]
fn no_building_goes_where_a_known_gun_or_army_reaches_it() {
    let quiet = under(|_| {});
    let (kind, first) = *quiet.first().expect("premise: a building is bought");
    let turret = first.offset(2, -2);
    let reach = BuildingKind::Turret.base_stats().weapons[0].range;
    let centre = (2 * i64::from(turret.x) + 1, 2 * i64::from(turret.y) + 1);
    assert!(
        within(kind, first, centre, reach),
        "premise: the gun reaches the first spot"
    );
    let gunned = under(|scenario| put(scenario, 1, BuildingKind::Turret, turret));
    assert!(!gunned.is_empty(), "premise: a building is still bought");
    for (kind, anchor) in &gunned {
        assert!(
            !within(*kind, *anchor, centre, reach),
            "{kind:?} at {anchor:?}"
        );
    }

    let bombard = TilePos::new(8, 8);
    let reach = defenses::reach(UnitKind::Bombard);
    let centre = (2 * i64::from(bombard.x) + 1, 2 * i64::from(bombard.y) + 1);
    assert!(within(kind, first, centre, reach), "premise");
    let shelled = under(|scenario| {
        scenario
            .units
            .push(unit(1, UnitKind::Bombard, bombard.x, bombard.y));
    });
    for (kind, anchor) in &shelled {
        assert!(
            !within(*kind, *anchor, centre, reach),
            "{kind:?} at {anchor:?}"
        );
    }
}

#[test]
fn a_gun_out_of_sight_changes_no_building() {
    // With the slots by its Foundry taken, West builds further east.
    let mut base = saturated(400);
    crowd(&mut base, 2, &[]);
    let quiet = under_in(base.clone(), |_| {});
    let (kind, first) = *quiet.first().expect("premise: a building is bought");
    let state = base.build().unwrap();
    let sight = ObservationData::fog_honest(&state, PlayerId(0));
    let reach = BuildingKind::Bastion.base_stats().weapons[0].range;
    let hidden = (1..10)
        .flat_map(|y| (12..21).map(move |x| TilePos::new(x, y)))
        .find(|anchor| {
            let centre = (2 * i64::from(anchor.x) + 2, 2 * i64::from(anchor.y) + 2);
            footprint(BuildingKind::Bastion, *anchor).all(|tile| {
                !sight.visible(tile)
                    && state.map().terrain_passable(tile)
                    && !base
                        .units
                        .iter()
                        .any(|unit| TilePos::new(unit.x, unit.y) == tile)
            }) && within(kind, first, centre, reach)
        })
        .expect("premise: an unseen place in reach of the first spot");
    let mut hiding = base.clone();
    put(&mut hiding, 1, BuildingKind::Bastion, hidden);
    let state = hiding.build().unwrap();
    assert!(
        ObservationData::fog_honest(&state, PlayerId(0))
            .enemy_buildings
            .is_empty(),
        "premise: West never saw it"
    );
    let unseen = under_in(base, |scenario| {
        put(scenario, 1, BuildingKind::Bastion, hidden)
    });
    assert_eq!(quiet, unseen);
}

#[test]
fn danger_covers_reach_minimum_range_and_recent_losses() {
    let scenario = saturated(400);
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let quiet = Danger::of(&observation, &model, &Memory::default());
    let spot = TilePos::new(9, 6);
    assert!(!quiet.hits(BuildingKind::Fabricator, spot));

    observation.salvage_incidents = vec![TilePos::new(14, 6)];
    let lost = Danger::of(&observation, &model, &Memory::default());
    assert!(lost.hits(BuildingKind::Fabricator, spot), "four tiles off");
    assert!(
        !lost.hits(BuildingKind::Fabricator, TilePos::new(7, 6)),
        "five tiles off"
    );

    let mut gunned = saturated(400);
    put(&mut gunned, 1, BuildingKind::Bastion, TilePos::new(12, 6));
    let state = gunned.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let danger = Danger::of(&observation, &model, &Memory::default());
    assert!(danger.hits(BuildingKind::Fabricator, TilePos::new(7, 6)));
    assert!(
        !danger.hits(BuildingKind::Reclaimer, TilePos::new(14, 6)),
        "inside its minimum range"
    );
    assert!(
        !danger.hits(BuildingKind::Fabricator, TilePos::new(14, 8)),
        "the nearest point inside its minimum range, as the gun measures"
    );
    assert!(
        !danger.hits(BuildingKind::Fabricator, TilePos::new(12, 20)),
        "out of range"
    );
}

#[test]
fn a_frame_within_a_known_guns_reach_waits() {
    let mut scenario = saturated(400);
    let frame = TilePos::new(9, 7);
    let mut row: Vec<char> = scenario.map[7].chars().collect();
    row[9] = 'E';
    scenario.map[7] = row.into_iter().collect();
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        !Danger::of(&observation, &model, &Memory::default()).hits(BuildingKind::Extractor, frame)
    );
    put(&mut scenario, 1, BuildingKind::Turret, TilePos::new(13, 6));
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        Danger::of(&observation, &model, &Memory::default()).hits(BuildingKind::Extractor, frame)
    );
}

#[test]
fn a_gun_behind_rock_cannot_hit_what_its_fire_cannot_reach() {
    let spot = TilePos::new(9, 6);
    let danger = |rock: bool| {
        let mut scenario = saturated(400);
        if rock {
            for row in 5..=7 {
                let mut cells: Vec<char> = scenario.map[row].chars().collect();
                cells[11] = '#';
                scenario.map[row] = cells.into_iter().collect();
            }
        }
        put(&mut scenario, 1, BuildingKind::Turret, TilePos::new(13, 6));
        let model = map(&scenario);
        let state = scenario.build().unwrap();
        let observation = ObservationData::fog_honest(&state, PlayerId(0));
        assert!(
            !observation.enemy_buildings.is_empty(),
            "premise: West sees the gun"
        );
        Danger::of(&observation, &model, &Memory::default()).hits(BuildingKind::Reclaimer, spot)
    };
    assert!(danger(false), "premise: in range on open ground");
    assert!(!danger(true), "the rock stops its direct fire");
}

#[test]
fn a_charge_on_its_way_closes_no_path() {
    let mut scenario = saturated(400);
    scenario.map = MOUTH.map(str::to_owned).to_vec();
    for unit in &mut scenario.units {
        if (unit.x, unit.y) == (9, 10) {
            (unit.x, unit.y) = (4, 9);
        }
    }
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let charges = [
        (BuildingKind::ScuttleCharge, TilePos::new(9, 6)),
        (BuildingKind::ScuttleCharge, TilePos::new(9, 7)),
    ];
    let keeps = |planned: &[(BuildingKind, TilePos)]| {
        defenses::keeps_paths(
            &observation,
            &model,
            BuildingKind::Reclaimer,
            TilePos::new(4, 2),
            planned,
        )
    };
    assert!(keeps(&[]), "premise");
    assert!(keeps(&charges), "charges across the corridor block nothing");
    let walls = charges.map(|(_, tile)| (BuildingKind::Barricade, tile));
    assert!(!keeps(&walls), "premise: Barricades there would");
}

#[test]
fn a_foundry_ringed_shut_keeps_no_path() {
    let mut scenario = arena(400);
    scenario
        .buildings
        .extend(
            crate::frame::ring(TilePos::new(3, 5), (2, 2)).map(|tile| BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: tile.x,
                y: tile.y,
            }),
        );
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(!defenses::keeps_paths(
        &observation,
        &model,
        BuildingKind::Reclaimer,
        TilePos::new(12, 5),
        &[]
    ));
}

#[test]
fn mirrored_seats_pack_mirrored_bases() {
    let mut scenario = saturated(400);
    let first = TilePos::new(7, 6);
    let rotate = |kind: BuildingKind, anchor: TilePos| {
        let (width, height) = kind.base_stats().size;
        TilePos::new(24 - width - anchor.x, 12 - height - anchor.y)
    };
    put(&mut scenario, 0, BuildingKind::Crucible, first);
    put(
        &mut scenario,
        1,
        BuildingKind::Crucible,
        rotate(BuildingKind::Crucible, first),
    );
    let state = scenario.build().unwrap();
    let west = builds(&seat(&scenario, 0).act(&state, &mut OwnEvents::default()));
    let east = builds(&seat(&scenario, 1).act(&state, &mut OwnEvents::default()));
    assert!(!west.is_empty(), "premise: West builds");
    let mirrored: Vec<(BuildingKind, TilePos)> = west
        .into_iter()
        .map(|(kind, anchor)| (kind, rotate(kind, anchor)))
        .collect();
    assert_eq!(mirrored, east);
}

#[test]
fn a_footprint_that_closes_off_a_pocket_is_refused() {
    let mut scenario = saturated(400);
    let pocket = TilePos::new(13, 7);
    for side in [(-1, 0), (1, 0), (0, -1)] {
        put(
            &mut scenario,
            0,
            BuildingKind::Reclaimer,
            pocket.offset(side.0, side.1),
        );
    }
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let keeps = |anchor: TilePos| {
        defenses::keeps_paths(&observation, &model, BuildingKind::Reclaimer, anchor, &[])
    };
    assert!(keeps(pocket.offset(0, 2)), "the pocket still opens below");
    assert!(!keeps(pocket.offset(0, 1)), "the last side closes it");
}
