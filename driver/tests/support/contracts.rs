//! Small rule scenarios whose assertions establish what each hash exercises.

use chassis::grid::TilePos;
use oxide_kit::GameReplay;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{
    AttackTarget, BuildingId, BuildingKind, Command, Event, Faction, GameResult, Order,
    PlayerCommand, PlayerId, Scenario, State, Target, UnitId, UnitKind,
};
use std::collections::BTreeMap;

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn arena(name: &str, units: Vec<UnitSpec>) -> Scenario {
    let mut rows = vec![vec!['.'; 40]; 30];
    rows[1][1] = '1';
    rows[27][37] = '2';
    Scenario {
        name: name.into(),
        seed: 42,
        map: rows
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .map(|faction| PlayerSpec {
                name: format!("{faction:?}"),
                faction,
                team: None,
                scrap: 4_000,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn terrain(scenario: &mut Scenario, at: TilePos, cell: char) {
    let mut row: Vec<_> = scenario.map[at.y as usize].chars().collect();
    row[at.x as usize] = cell;
    scenario.map[at.y as usize] = row.into_iter().collect();
}

struct Probe {
    state: State,
    twin: State,
    restored: State,
    replay: GameReplay,
    events: Vec<Event>,
    ticks: Vec<u64>,
    milestones: BTreeMap<String, String>,
}

impl Probe {
    fn new(scenario: Scenario) -> Self {
        Self {
            state: scenario.build().unwrap(),
            twin: scenario.build().unwrap(),
            restored: scenario.build().unwrap(),
            replay: GameReplay::new(oxide_sim::SIM_VERSION, scenario),
            events: Vec::new(),
            ticks: Vec::new(),
            milestones: BTreeMap::new(),
        }
    }

    fn building(&self, player: u8, kind: BuildingKind) -> BuildingId {
        self.state
            .buildings()
            .iter()
            .find(|b| b.player == PlayerId(player) && b.kind == kind)
            .unwrap()
            .id
    }

    fn unit(&self, player: u8, kind: UnitKind) -> UnitId {
        self.state
            .units()
            .iter()
            .find(|u| u.player == PlayerId(player) && u.kind == kind)
            .unwrap()
            .id
    }

    fn command(&mut self, player: u8, command: Command) {
        self.step(
            &[PlayerCommand {
                player: PlayerId(player),
                command,
            }],
            false,
        );
    }

    fn step(&mut self, commands: &[PlayerCommand], rejected: bool) {
        let tick = self.state.current_tick();
        for command in commands {
            self.replay.record(tick, command.clone());
        }
        let report = self.state.tick(commands);
        let other = self.twin.tick(commands);
        let continued = self.restored.tick(commands);
        assert_eq!(report, other, "independent events at {tick}");
        assert_eq!(report, continued, "restored events at {tick}");
        assert_eq!(
            self.state.hash(),
            self.twin.hash(),
            "independent state at {tick}"
        );
        assert_eq!(
            self.state.hash(),
            self.restored.hash(),
            "restored state at {tick}"
        );
        assert_eq!(
            report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. })),
            rejected,
            "unexpected command disposition at {tick}: {:?}",
            report.events
        );
        self.state.validate_invariants().unwrap();
        self.events.extend(report.events);
        self.ticks.push(self.state.hash());
    }

    fn until(&mut self, label: &str, limit: u64, condition: impl Fn(&Self) -> bool) {
        for _ in 0..limit {
            if condition(self) {
                return;
            }
            self.step(&[], false);
        }
        assert!(condition(self), "{label} not reached within {limit} ticks");
    }

    fn mark(&mut self, label: &str) {
        let key = format!("{}#{label}", self.replay.setup.name);
        assert!(
            self.milestones
                .insert(key, oxide_protocol::hash_hex(self.state.hash()))
                .is_none()
        );
        self.restored = serde_json::from_slice(&serde_json::to_vec(&self.state).unwrap()).unwrap();
        self.restored.validate_invariants().unwrap();
        assert_eq!(self.state.hash(), self.restored.hash());
    }

    fn finish(mut self) -> BTreeMap<String, String> {
        self.replay.meta.ticks = Some(self.state.current_tick());
        let replay: GameReplay =
            serde_json::from_slice(&serde_json::to_vec(&self.replay).unwrap()).unwrap();
        replay.validate(Some(oxide_sim::SIM_VERSION)).unwrap();
        let mut state = replay.setup.build().unwrap();
        let mut cursor = replay.cursor();
        for (tick, hash) in self.ticks.iter().enumerate() {
            let commands: Vec<_> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|row| row.command.clone())
                .collect();
            state.tick(&commands);
            assert_eq!(
                state.hash(),
                *hash,
                "recorded replay diverged at step {tick}"
            );
        }
        assert!(cursor.is_finished());
        self.milestones
    }
}

fn economy() -> BTreeMap<String, String> {
    let mut scenario = arena("economy", vec![unit(0, UnitKind::Harvester, 4, 3)]);
    terrain(&mut scenario, TilePos::new(5, 3), 's');
    let mut p = Probe::new(scenario);
    let foundry = p.building(0, BuildingKind::Foundry);
    let bank = p.state.player(PlayerId(0)).scrap;
    p.command(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    );
    assert_eq!(
        p.state.player(PlayerId(0)).scrap,
        bank - UnitKind::Harvester.stats().cost
    );
    p.command(
        0,
        Command::CancelTrain {
            building: foundry,
            index: 0,
        },
    );
    assert!(p.state.building(foundry).unwrap().queue.is_empty());
    assert_eq!(p.state.player(PlayerId(0)).scrap, bank);
    p.mark("refund");
    p.command(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    );
    p.until("trained worker", 200, |p| {
        p.events
            .iter()
            .any(|e| matches!(e, Event::UnitTrained { building, .. } if *building == foundry))
    });
    let worker = p.unit(0, UnitKind::Harvester);
    p.command(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(5, 3),
            queue: false,
        },
    );
    p.until("harvest delivery", 600, |p| p.events.iter().any(|e| matches!(e, Event::ScrapDeposited { amount, player: PlayerId(0), .. } if *amount > 0)));
    p.mark("delivery");
    p.finish()
}

fn construction() -> BTreeMap<String, String> {
    let mut scenario = arena("construction", vec![unit(0, UnitKind::Harvester, 5, 6)]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 3,
        y: 10,
    });
    let mut p = Probe::new(scenario);
    let worker = p.unit(0, UnitKind::Harvester);
    let anchor = TilePos::new(6, 6);
    let build = Command::Build {
        units: vec![worker],
        kind: BuildingKind::Turret,
        anchor,
        queue: false,
        defer: false,
    };
    let bank = p.state.player(PlayerId(0)).scrap;
    p.command(0, build.clone());
    let site = p.building(0, BuildingKind::Turret);
    assert_eq!(
        p.state.player(PlayerId(0)).scrap,
        bank - BuildingKind::Turret.base_stats().construction.unwrap().cost
    );
    assert!(!p.state.building(site).unwrap().built);
    p.until("construction progress", 100, |p| {
        p.state.building(site).unwrap().progress > 0
    });
    p.command(
        0,
        Command::Stop {
            units: vec![worker],
        },
    );
    let progress = p.state.building(site).unwrap().progress;
    for _ in 0..10 {
        p.step(&[], false);
    }
    assert_eq!(p.state.building(site).unwrap().progress, progress);
    p.mark("interrupted");
    let bank = p.state.player(PlayerId(0)).scrap;
    p.command(0, build);
    assert_eq!(p.state.player(PlayerId(0)).scrap, bank);
    p.until("resumed construction", 600, |p| {
        p.state.building(site).unwrap().built
    });
    p.mark("completed");
    let bank = p.state.player(PlayerId(0)).scrap;
    let upgrade = BuildingKind::Turret.upgrade_from(0).unwrap();
    p.command(0, Command::UpgradeBuilding { building: site });
    assert!(!p.state.building(site).unwrap().built);
    assert_eq!(p.state.player(PlayerId(0)).scrap, bank - upgrade.cost);
    p.until("upgrade completion", u64::from(upgrade.build_ticks), |p| {
        p.state.building(site).unwrap().built
    });
    assert_eq!(p.state.building(site).unwrap().tier, 1);
    p.mark("upgraded");
    p.command(
        0,
        Command::Salvage {
            units: vec![worker],
            building: site,
            queue: false,
        },
    );
    p.until("building salvage", 2000, |p| {
        p.events
            .iter()
            .any(|e| matches!(e, Event::BuildingSalvaged { building, .. } if *building == site))
    });
    assert!(p.state.building(site).is_none());
    p.mark("salvaged");
    p.finish()
}

fn ground() -> BTreeMap<String, String> {
    let mut scenario = arena(
        "ground",
        vec![
            unit(0, UnitKind::Sentinel, 5, 10),
            unit(1, UnitKind::Harvester, 21, 10),
        ],
    );
    for y in 8..=12 {
        terrain(&mut scenario, TilePos::new(10, y), '#');
    }
    let mut p = Probe::new(scenario);
    let fighter = p.unit(0, UnitKind::Sentinel);
    let victim = p.unit(1, UnitKind::Harvester);
    p.command(
        0,
        Command::Move {
            units: vec![fighter],
            goal: TilePos::new(14, 10),
            queue: false,
        },
    );
    p.command(
        0,
        Command::Move {
            units: vec![fighter],
            goal: TilePos::new(17, 10),
            queue: true,
        },
    );
    assert!(!p.state.unit(fighter).unwrap().queue.is_empty());
    p.until("obstacle route and queued arrival", 1000, |p| {
        p.state
            .unit(fighter)
            .is_some_and(|u| u.tile() == TilePos::new(17, 10) && u.order == Order::Idle)
    });
    p.mark("arrival");
    let hp = p.state.unit(victim).unwrap().hp;
    p.command(
        0,
        Command::Attack {
            units: vec![fighter],
            target: Target::Unit(victim).into(),
            queue: false,
        },
    );
    p.until("ground damage", 200, |p| {
        p.state.unit(victim).is_none_or(|u| u.hp < hp)
    });
    p.mark("damage");
    p.until("ground casualty", 1000, |p| p.state.unit(victim).is_none());
    assert!(
        p.events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == victim))
    );
    assert!(p.state.map().wreck_at(TilePos::new(21, 10)) > 0);
    p.mark("wreck");
    p.finish()
}

fn air() -> BTreeMap<String, String> {
    let scenario = arena(
        "air",
        vec![
            unit(0, UnitKind::Skyhook, 6, 6),
            unit(0, UnitKind::Sentinel, 5, 6),
            unit(0, UnitKind::Condor, 10, 15),
            unit(1, UnitKind::Harvester, 25, 15),
        ],
    );
    let mut p = Probe::new(scenario);
    let carrier = p.unit(0, UnitKind::Skyhook);
    let rider = p.unit(0, UnitKind::Sentinel);
    let bomber = p.unit(0, UnitKind::Condor);
    let victim = p.unit(1, UnitKind::Harvester);
    p.command(
        0,
        Command::Load {
            units: vec![rider],
            transport: carrier,
            queue: false,
        },
    );
    p.until("boarding", 300, |p| {
        p.events
            .iter()
            .any(|e| matches!(e, Event::UnitBoarded { unit, .. } if *unit == rider))
    });
    assert!(p.state.unit(rider).is_none());
    p.mark("boarded");
    p.command(
        0,
        Command::Unload {
            transport: carrier,
            at: TilePos::new(19, 6),
            queue: false,
        },
    );
    p.until("delivery", 1000, |p| p.events.iter().any(|e| matches!(e, Event::UnitUnloaded { unit, at, .. } if *unit == rider && at.chebyshev(TilePos::new(19, 6)) <= 4)));
    assert!(p.state.unit(rider).is_some());
    p.mark("delivered");
    p.command(
        0,
        Command::Move {
            units: vec![bomber],
            goal: TilePos::new(19, 15),
            queue: false,
        },
    );
    p.until("bomber landing", 1500, |p| {
        p.state.unit(bomber).unwrap().landed
    });
    assert!(
        p.state
            .unit(bomber)
            .unwrap()
            .tile()
            .chebyshev(TilePos::new(19, 15))
            <= 1
    );
    p.mark("landed");
    assert!(
        p.state
            .can_see(PlayerId(0), p.state.unit(victim).unwrap().tile())
    );
    p.command(
        0,
        Command::Attack {
            units: vec![bomber],
            target: Target::Unit(victim).into(),
            queue: false,
        },
    );
    p.until("bomber takeoff", 100, |p| {
        !p.state.unit(bomber).unwrap().landed
    });
    p.until("bomber release", 1500, |p| p.events.iter().any(|e| matches!(e, Event::ShellLaunched { shooter: Target::Unit(id), .. } if *id == bomber)));
    p.until("bomber damage", 200, |p| {
        p.state
            .unit(victim)
            .is_none_or(|u| u.hp < UnitKind::Harvester.stats().max_hp)
    });
    p.mark("attack");
    p.finish()
}

fn fog() -> BTreeMap<String, String> {
    let mut scenario = arena(
        "fog",
        vec![
            unit(0, UnitKind::Gnat, 16, 6),
            unit(1, UnitKind::Gnat, 14, 8),
        ],
    );
    scenario.buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 5,
            y: 16,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 5,
            y: 6,
        },
        BuildingSpec {
            player: 1,
            kind: BuildingKind::Reclaimer,
            x: 18,
            y: 6,
        },
    ];
    let mut p = Probe::new(scenario);
    let scout = p.unit(0, UnitKind::Gnat);
    let gun = p.building(0, BuildingKind::Bastion);
    let site = TilePos::new(18, 6);
    assert!(p.state.can_see(PlayerId(0), site));
    p.command(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 27),
            queue: false,
        },
    );
    p.until("lost building sight", 1000, |p| {
        !p.state.can_see(PlayerId(0), site)
    });
    assert!(
        p.state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .any(|g| g.anchor == site)
    );
    p.mark("remembered");
    let remembered = p
        .state
        .vision(PlayerId(0))
        .ghosts()
        .iter()
        .find(|g| g.anchor == site)
        .unwrap();
    let target = AttackTarget::RememberedBuilding(oxide_sim::RememberedBuilding {
        owner: remembered.owner,
        building_kind: remembered.kind,
        anchor: remembered.anchor,
    });
    p.command(
        0,
        Command::FocusFire {
            buildings: vec![gun],
            target,
        },
    );
    assert_eq!(p.state.building(gun).unwrap().focus, Some(target));
    p.command(
        0,
        Command::ClearFocus {
            buildings: vec![gun],
        },
    );
    p.until("anonymous radar track", 1000, |p| {
        p.state
            .vision(PlayerId(0))
            .tracks()
            .iter()
            .any(|t| t.visible_unit.is_none())
    });
    let contact = p
        .state
        .vision(PlayerId(0))
        .tracks()
        .iter()
        .find(|t| t.visible_unit.is_none())
        .unwrap()
        .id;
    let before_focus = p.events.len();
    p.command(
        0,
        Command::FocusFire {
            buildings: vec![gun],
            target: AttackTarget::Contact(contact),
        },
    );
    assert_eq!(
        p.state.building(gun).unwrap().focus,
        Some(AttackTarget::Contact(contact))
    );
    p.until("blind shell", 200, |p| {
        p.events[before_focus..].iter().any(|e| {
            matches!(e,
        Event::ShellLaunched { shooter: Target::Building(id), target: None, .. } if *id == gun)
        })
    });

    p.mark("radar-fire");
    p.step(
        &[PlayerCommand {
            player: PlayerId(1),
            command: Command::FocusFire {
                buildings: vec![gun],
                target: AttackTarget::Contact(contact),
            },
        }],
        true,
    );
    p.mark("foreign-command-refused");
    p.finish()
}

fn teams() -> BTreeMap<String, String> {
    let mut scenario = arena("teams", vec![unit(0, UnitKind::Gnat, 12, 12)]);
    scenario.players[0].team = Some(0);
    scenario.players[1].team = Some(0);
    scenario.players.push(PlayerSpec {
        name: "Enemy".into(),
        faction: Faction::Ferrous,
        team: Some(1),
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    terrain(&mut scenario, TilePos::new(37, 1), '3');
    let mut p = Probe::new(scenario);
    assert!(p.state.can_see(PlayerId(1), TilePos::new(12, 12)));
    p.mark("shared-sight");
    p.command(0, Command::Surrender);
    assert!(p.state.player(PlayerId(0)).eliminated_at.is_some());
    assert!(p.state.result().is_none());
    p.mark("ally-survives");
    p.command(2, Command::Surrender);
    assert_eq!(p.state.result(), Some(GameResult::Victory { team: 0 }));
    p.mark("victory");
    let mut frozen = serde_json::to_value(&p.state).unwrap();
    let events = p.events.len();
    let tick = p.state.current_tick();
    p.command(
        1,
        Command::Train {
            building: p.building(1, BuildingKind::Foundry),
            kind: UnitKind::Harvester,
        },
    );
    for _ in 0..2 {
        p.step(&[], false);
    }
    assert_eq!(p.state.current_tick(), tick + 3);
    assert_eq!(p.events.len(), events);
    frozen["tick"] = serde_json::json!(tick + 3);
    assert_eq!(serde_json::to_value(&p.state).unwrap(), frozen);
    p.mark("post-result");
    p.finish()
}

pub fn compute_hashes() -> BTreeMap<String, String> {
    [economy(), construction(), ground(), air(), fog(), teams()]
        .into_iter()
        .flatten()
        .collect()
}

#[test]
#[should_panic(expected = "unreachable milestone not reached")]
fn a_missing_behavior_cannot_be_blessed() {
    let mut p = Probe::new(arena("missing", vec![]));
    p.until("unreachable milestone", 2, |_| false);
}

#[test]
#[should_panic(expected = "unexpected command disposition")]
fn an_unexpected_rejection_cannot_be_blessed() {
    let mut p = Probe::new(arena("rejected", vec![]));
    let foreign = p.building(1, BuildingKind::Foundry);
    p.command(
        0,
        Command::Train {
            building: foreign,
            kind: UnitKind::Harvester,
        },
    );
}
