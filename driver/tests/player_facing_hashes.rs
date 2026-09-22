//! Focused controller contracts with cross-platform state and command hashes.
//! Behavioral assertions must pass before a checkpoint can be blessed.

mod support;

use chassis::grid::TilePos;
use oxide_bot::{SeatBot, seat_bots};
use oxide_kit::GameReplay;
use oxide_sim::scenario::{BotConfig, PlayerSpec, UnitSpec};
use oxide_sim::{
    Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, TickReport, UnitKind,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn scenario(name: &str, scrap: u32, worker: bool) -> Scenario {
    let mut map = vec![vec!['.'; 28]; 16];
    map[2][2] = '1';
    map[12][24] = '2';
    map[4][6] = 's';
    Scenario {
        name: name.into(),
        seed: 42,
        map: map
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: vec![
            PlayerSpec {
                name: "Controller".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap,
                bot: true,
                bot_config: Some(BotConfig::default()),
            },
            PlayerSpec {
                name: "Idle opponent".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: if worker {
            vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 4,
            }]
        } else {
            Vec::new()
        },
        buildings: Vec::new(),
        meta: None,
    }
}

struct Probe {
    state: State,
    twin: State,
    bots: Vec<SeatBot>,
    twin_bots: Vec<SeatBot>,
    replay: GameReplay,
    history: Vec<(u64, TickReport)>,
    command_fold: u64,
}

impl Probe {
    fn new(scenario: Scenario) -> Self {
        let state = scenario.build().unwrap();
        state.validate_invariants().unwrap();
        let bots = seat_bots(&scenario).unwrap();
        assert_eq!(bots.len(), 1);
        Self {
            twin: scenario.build().unwrap(),
            state,
            bots,
            twin_bots: seat_bots(&scenario).unwrap(),
            replay: GameReplay::new(oxide_sim::SIM_VERSION, scenario),
            history: Vec::new(),
            command_fold: 0,
        }
    }

    fn step(&mut self) -> (Vec<PlayerCommand>, TickReport) {
        let tick = self.state.current_tick();
        let commands: Vec<_> = self
            .bots
            .iter_mut()
            .flat_map(|bot| bot.act(&self.state))
            .collect();
        let other: Vec<_> = self
            .twin_bots
            .iter_mut()
            .flat_map(|bot| bot.act(&self.twin))
            .collect();
        assert_eq!(commands, other, "controller commands diverged at {tick}");
        for command in &commands {
            self.replay.record(tick, command.clone());
        }
        self.command_fold = chassis::hash::state_hash(&(self.command_fold, tick, &commands));
        let mut restored: State =
            serde_json::from_slice(&serde_json::to_vec(&self.state).unwrap()).unwrap();
        let report = self.state.tick(&commands);
        assert_eq!(
            report,
            self.twin.tick(&commands),
            "independent events at {tick}"
        );
        assert_eq!(
            report,
            restored.tick(&commands),
            "restored events at {tick}"
        );
        assert_eq!(
            self.state.hash(),
            self.twin.hash(),
            "independent state at {tick}"
        );
        assert_eq!(
            self.state.hash(),
            restored.hash(),
            "restored state at {tick}"
        );
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, Event::CommandRejected { .. })),
            "focused controller issued an invalid command at {tick}: {:?}",
            report.events
        );
        self.state.validate_invariants().unwrap();
        self.history.push((self.state.hash(), report.clone()));
        (commands, report)
    }

    fn finish(mut self) -> BTreeMap<String, String> {
        self.replay.meta.ticks = Some(self.state.current_tick());
        let replay: GameReplay =
            serde_json::from_slice(&serde_json::to_vec(&self.replay).unwrap()).unwrap();
        replay.validate(Some(oxide_sim::SIM_VERSION)).unwrap();
        let mut state = replay.setup.build().unwrap();
        let mut cursor = replay.cursor();
        for (hash, report) in self.history {
            let commands: Vec<_> = cursor
                .take_tick(state.current_tick())
                .iter()
                .map(|row| row.command.clone())
                .collect();
            assert_eq!(state.tick(&commands), report, "replay events diverged");
            assert_eq!(state.hash(), hash, "replay state diverged");
        }
        assert!(cursor.is_finished());
        BTreeMap::from([
            (
                replay.setup.name.clone(),
                oxide_protocol::hash_hex(state.hash()),
            ),
            (
                format!("{}#commands", replay.setup.name),
                oxide_protocol::hash_hex(self.command_fold),
            ),
        ])
    }
}

fn recovery(funded: bool) -> BTreeMap<String, String> {
    let price = UnitKind::Harvester.stats().cost;
    let name = if funded {
        "recovery-funded"
    } else {
        "recovery-underfunded"
    };
    let bank = if funded { price } else { price - 1 };
    let mut probe = Probe::new(scenario(name, bank, false));
    assert!(probe.state.units().is_empty());
    let foundry = probe
        .state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0))
        .unwrap()
        .id;
    let (commands, _) = probe.step();
    let expected = if funded {
        vec![PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        }]
    } else {
        Vec::new()
    };
    assert_eq!(
        commands, expected,
        "recovery must respect the exact worker price"
    );
    // A queued worker stops recovery income; the unfunded seat earns one tick-zero credit.
    assert_eq!(
        probe.state.player(PlayerId(0)).scrap,
        if funded { 0 } else { bank + 1 }
    );
    let queue = &probe.state.building(foundry).unwrap().queue;
    assert_eq!(queue.len(), usize::from(funded));
    if funded {
        assert_eq!(queue[0], UnitKind::Harvester);
    }
    probe.finish()
}

fn harvesting() -> BTreeMap<String, String> {
    let mut probe = Probe::new(scenario("harvest-repeat-delivery", 0, true));
    let worker = probe.state.units()[0].id;
    let node = TilePos::new(6, 4);
    assert_eq!(probe.state.unit(worker).unwrap().order, Order::Idle);
    assert!(probe.state.can_see(PlayerId(0), node));
    assert!(probe.state.map().scrap_at(node) > 0);
    let (commands, _) = probe.step();
    assert!(commands.iter().any(|command| matches!(&command.command,
        Command::Harvest { units, node: target, .. } if units.contains(&worker) && *target == node
    )), "idle worker did not receive harvesting work: {commands:?}");
    let mut deliveries = 0;
    let mut deposited = 0;
    for _ in 0..600 {
        let (_, report) = probe.step();
        for event in report.events {
            if let Event::ScrapDeposited {
                player: PlayerId(0),
                amount,
            } = event
            {
                assert!(amount > 0);
                deliveries += 1;
                deposited += amount;
            }
        }
        if deliveries == 2 {
            break;
        }
    }
    assert_eq!(
        deliveries, 2,
        "worker failed to complete two nearby harvest cycles"
    );
    assert_eq!(probe.state.player(PlayerId(0)).scrap, deposited);
    assert!(matches!(
        probe.state.unit(worker).unwrap().order,
        Order::Harvest { .. }
    ));
    probe.finish()
}

#[test]
fn focused_controller_contracts_match_hash_fixtures() {
    let actual = [recovery(false), recovery(true), harvesting()]
        .into_iter()
        .flatten()
        .collect();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/player-facing-hashes.json");
    support::check_or_bless(&fixture, actual);
}
