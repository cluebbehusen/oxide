use super::*;
use crate::expansion;
use crate::investments::Investment;
use crate::map::Site;
use crate::memory::Memory;

/// A half-turn-symmetric frontier. Each seat has two home nodes, a small safe
/// field eight tiles out, and a rich field beside the other seat's start.
pub(super) const FRONTIER: [&str; 14] = [
    "########################################",
    "############...........................#",
    "#...ss......SS............ss...........#",
    "#...........SS............ss...........#",
    "#......................................#",
    "#......................................#",
    "#.1.................................2..#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#...........ss............SS...........#",
    "#...........ss............SS......ss...#",
    "#...........................############",
    "########################################",
];

fn frontier(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.map = FRONTIER.map(str::to_owned).to_vec();
    scenario.units = vec![
        harvester(0, 4, 3),
        harvester(0, 5, 3),
        harvester(1, 35, 10),
        harvester(1, 34, 10),
    ];
    scenario
}

fn rotate(tile: TilePos) -> TilePos {
    TilePos::new(40 - 1 - tile.x, 14 - 1 - tile.y)
}

/// The site whose field holds `node`.
fn site_with(model: &MapModel, node: TilePos) -> (usize, &Site) {
    model
        .sites()
        .iter()
        .enumerate()
        .find(|(_, site)| site.nodes.iter().any(|(tile, _)| *tile == node))
        .expect("a site holds the node")
}

#[test]
fn sites_and_distances_mirror_between_seats() {
    let model = map(&frontier(0));
    assert!(model.sites().len() >= 4, "home fields are not sites");
    for site in model.sites() {
        let (_, mirror) = site_with(&model, rotate(site.nodes[0].0));
        let rotated: Vec<TilePos> = site
            .anchors
            .iter()
            .map(|anchor| TilePos::new(40 - 2 - anchor.x, 14 - 2 - anchor.y))
            .collect();
        assert_eq!(rotated, mirror.anchors);
        assert!(!site.anchors.is_empty());
    }
    for y in 0..14 {
        for x in 0..40 {
            let tile = TilePos::new(x, y);
            assert_eq!(
                model.distance(PlayerId(0), tile),
                model.distance(PlayerId(1), rotate(tile)),
                "{tile:?}"
            );
        }
    }
}

#[test]
fn greed_trades_a_safe_field_for_a_richer_contested_one() {
    let scenario = frontier(0);
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::omniscient(&state, PlayerId(0));
    let memory = Memory::default();
    let (_, safe) = site_with(&model, TilePos::new(12, 10));
    let (_, rich) = site_with(&model, TilePos::new(26, 10));
    let value =
        |site: &Site, greed| expansion::value(&observation, &model, &memory, site, greed).unwrap();
    assert!(value(safe, 0) > value(rich, 0));
    assert!(value(rich, 100) > value(safe, 100));
}

#[test]
fn a_site_that_failed_at_every_anchor_is_skipped() {
    let scenario = frontier(0);
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::omniscient(&state, PlayerId(0));
    let (_, safe) = site_with(&model, TilePos::new(12, 10));
    let mut memory = Memory::default();
    for anchor in &safe.anchors[1..] {
        memory.fail(BuildingKind::Foundry, *anchor, 0);
    }
    assert!(expansion::value(&observation, &model, &memory, safe, 50).is_some());
    memory.fail(BuildingKind::Foundry, safe.anchors[0], 0);
    assert_eq!(
        expansion::value(&observation, &model, &memory, safe, 50),
        None
    );
}

#[test]
fn an_unexplored_expansion_sends_a_harvester_to_look() {
    let mut scenario = frontier(1_000);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 6,
        y: 8,
    });
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let (index, far) = site_with(&model, TilePos::new(26, 10));
    assert!(
        far.anchors
            .iter()
            .all(|anchor| !observation.explored(*anchor)),
        "premise: the far field is in fog"
    );
    let nearer: Vec<serde_json::Value> = [TilePos::new(12, 2), TilePos::new(12, 10)]
        .into_iter()
        .flat_map(|node| site_with(&model, node).1.anchors.clone())
        .map(|anchor| serde_json::json!({"kind": "foundry", "anchor": anchor, "at": 0}))
        .collect();
    let mut checkpoint = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    checkpoint["memory"]["failures"] = serde_json::Value::Array(nearer);
    checkpoint["saving"] = serde_json::json!({
        "protected": 0,
        "target": {"investment": {"expansion": index}, "attempt": null},
    });
    let checkpoint: Checkpoint = serde_json::from_value(checkpoint).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, model.clone()).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert_eq!(
        trace.unwrap().target.map(|target| target.investment),
        Some(Investment::Expansion(index as u16))
    );
    assert!(builds_of(&commands).is_empty(), "nothing to place in fog");
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Run { goal, .. } if *goal == far.anchors[0]
        )),
        "{commands:?}"
    );
}

fn builds_of(commands: &[PlayerCommand]) -> Vec<BuildingKind> {
    commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Build { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

#[test]
fn harvesters_work_the_field_of_a_new_foundry() {
    let mut scenario = frontier(0);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 14,
        y: 8,
    });
    scenario.units = vec![harvester(0, 14, 11), harvester(0, 15, 11)];
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let field = [
        TilePos::new(12, 10),
        TilePos::new(13, 10),
        TilePos::new(12, 11),
        TilePos::new(13, 11),
    ];
    let sent: Vec<TilePos> = harvests(&commands)
        .into_iter()
        .map(|(node, _)| node)
        .collect();
    assert_eq!(sent.len(), 2, "{commands:?}");
    assert!(sent.iter().all(|node| field.contains(node)), "{sent:?}");
}

#[test]
fn a_free_frame_near_home_gets_an_extractor() {
    let mut scenario = arena(1_000);
    scenario.map[1].replace_range(6..7, "E");
    scenario
        .units
        .extend([harvester(0, 5, 7), harvester(0, 4, 7)]);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let mut events = OwnEvents::default();
    let frame = TilePos::new(6, 1);
    let mut placed = false;
    while state.current_tick() < 600 && !placed {
        let commands = opponent.act(&state, &mut events);
        placed = commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Build { kind: BuildingKind::Extractor, anchor, .. } if anchor == frame
            )
        });
        events.record(PlayerId(0), &state.tick(&commands).events);
    }
    assert!(placed);
}

#[test]
fn only_frames_the_seat_has_seen_add_to_a_site() {
    let mut scenario = frontier(0);
    scenario.map[9].replace_range(16..18, "E.");
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let (_, safe) = site_with(&model, TilePos::new(12, 10));
    assert!(safe.frames.contains(&TilePos::new(16, 9)), "premise");
    let mut observation = ObservationData::omniscient(&state, PlayerId(0));
    let memory = Memory::default();
    let seen = expansion::value(&observation, &model, &memory, safe, 50).unwrap();
    observation.known_frames.clear();
    let hidden = expansion::value(&observation, &model, &memory, safe, 50).unwrap();
    assert!(seen > hidden);
}

#[test]
fn a_site_is_valued_and_built_from_anchors_on_home_ground() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios/severance.json");
    let scenario = Scenario::load(path).unwrap();
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    for (player, node) in [
        (PlayerId(0), TilePos::new(30, 21)),
        (PlayerId(1), TilePos::new(33, 22)),
    ] {
        let observation = ObservationData::omniscient(&state, player);
        let (index, site) = site_with(&model, node);
        let home = model.start(player).and_then(|start| model.component(start));
        assert_ne!(
            model.component(site.anchors[0]),
            home,
            "premise: the best anchor is off home"
        );
        let reachable = expansion::anchors(&model, player, site);
        assert!(!reachable.is_empty(), "{player:?}");
        assert!(
            reachable
                .iter()
                .all(|anchor| model.component(*anchor) == home)
        );
        assert!(expansion::value(&observation, &model, &Memory::default(), site, 50).is_some());
        let placed = crate::investments::anchors(
            &model,
            &observation,
            Investment::Expansion(index as u16),
            BuildingKind::Foundry,
        );
        assert_eq!(placed, reachable);
    }
}

#[test]
fn a_half_explored_footprint_is_still_scouted() {
    let mut scenario = frontier(1_000);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 6,
        y: 8,
    });
    let model = map(&scenario);
    let state = scenario.build().unwrap();
    let (index, far) = site_with(&model, TilePos::new(26, 10));
    let nearer: Vec<serde_json::Value> = [TilePos::new(12, 2), TilePos::new(12, 10)]
        .into_iter()
        .flat_map(|node| site_with(&model, node).1.anchors.clone())
        .map(|anchor| serde_json::json!({"kind": "foundry", "anchor": anchor, "at": 0}))
        .collect();
    let mut checkpoint = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    checkpoint["memory"]["failures"] = serde_json::Value::Array(nearer);
    checkpoint["saving"] = serde_json::json!({
        "protected": 0,
        "target": {"investment": {"expansion": index}, "attempt": null},
    });
    let checkpoint: Checkpoint = serde_json::from_value(checkpoint).unwrap();
    let opponent = Opponent::restore(&checkpoint, &scenario, &state, model.clone()).unwrap();

    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    let anchor = far.anchors[0];
    let index_of =
        |tile: TilePos| usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
    let explored = index_of(anchor);
    observation.explored[explored] = true;
    let mut persistent = opponent.persistent.clone();
    let decision = crate::decision::decide(
        &observation,
        false,
        &model,
        opponent.profile(),
        &mut persistent,
    );
    assert!(
        decision.commands.iter().any(|command| matches!(
            &command.command,
            Command::Run { goal, .. } if *goal == anchor
        )),
        "{:?}",
        decision.commands
    );
}
