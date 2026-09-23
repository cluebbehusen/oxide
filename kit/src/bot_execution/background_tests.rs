use super::*;
use crate::checkpoint::SessionCheckpoint;
use oxide_bot::seat_bots;
use oxide_sim::{
    Command, PlayerId, Scenario,
    scenario::{BotConfig, BotDifficulty, BotStance},
};
use std::sync::Barrier;

fn checkpoint(bots: &[SeatBot]) -> Vec<u8> {
    serde_json::to_vec(
        &bots
            .iter()
            .map(|b| b.checkpoint().unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

// Occupy the existing pool without sleeps or production dispatch hooks.
fn block(executor: &BotExecutor) -> Arc<Barrier> {
    let pool = executor.pool.as_ref().unwrap();
    let entered = Arc::new(Barrier::new(pool.current_num_threads() + 1));
    let release = Arc::new(Barrier::new(pool.current_num_threads() + 1));
    for _ in 0..pool.current_num_threads() {
        let entered = entered.clone();
        let release = release.clone();
        pool.spawn(move || {
            entered.wait();
            release.wait();
        });
    }
    entered.wait();
    release
}

#[test]
fn snapshots_save_and_recover_while_running_and_ready() {
    let scenario = Scenario::skirmish();
    let executor = BotExecutor::new(2);
    let mut state = Arc::new(scenario.build().unwrap());
    let mut bots = seat_bots(&scenario).unwrap();
    for _ in 0..120 {
        let commands = executor.commands(&state, &mut bots);
        Arc::get_mut(&mut state).unwrap().tick(&commands);
    }
    let human = vec![PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: oxide_sim::BuildingId(0),
            kind: oxide_sim::UnitKind::Harvester,
        },
    }];
    let capture = |bots: &[SeatBot]| {
        SessionCheckpoint::capture(&scenario, &state, bots, &human, None).unwrap()
    };
    let idle = capture(&bots);
    let before = checkpoint(&bots);
    let release = block(&executor);
    let mut job = executor.prepare(&state, &bots, None).unwrap();
    let running = capture(&bots);
    assert_eq!(checkpoint(&bots), before);
    assert!(matches!(
        job.result.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(executor.prepare(&state, &bots, None).is_none());
    let mut concurrent = bots.clone();
    assert_eq!(
        executor.commands(&state, &mut concurrent),
        serial_commands(&state, &mut bots.clone(), None)
    );
    release.wait();
    let ready = job.result.recv().unwrap();
    let (send, receive) = mpsc::channel();
    send.send(ready).unwrap();
    job.result = receive;
    assert_eq!(Arc::strong_count(&state), 1);
    let prepared = capture(&bots);
    assert_eq!(checkpoint(&bots), before);
    let mut replay = prepared.recording().unwrap();
    let mut commands = human.clone();
    commands.extend(job.finish(&state, &mut bots, None));
    for command in &commands {
        replay.record(state.current_tick(), command.clone());
    }
    Arc::get_mut(&mut state).unwrap().tick(&commands);
    replay.meta.ticks = Some(state.current_tick());
    for saved in [idle, running, prepared] {
        let empty = saved.recording().unwrap();
        let mut restored = saved.clone().resume_recording(&empty).unwrap();
        assert_eq!(restored.pending, human);
        let mut restored_commands = std::mem::take(&mut restored.pending);
        restored_commands.extend(executor.commands(&restored.state, &mut restored.bots));
        assert_eq!(commands, restored_commands);
        restored.state.tick(&restored_commands);
        let mut recovered = saved.resume_recording(&replay).unwrap();
        assert!(recovered.pending.is_empty());
        assert_eq!(restored.state.hash(), state.hash());
        assert_eq!(recovered.state.hash(), state.hash());
        assert_eq!(checkpoint(&restored.bots), checkpoint(&bots));
        assert_eq!(checkpoint(&recovered.bots), checkpoint(&bots));
        for _ in 0..60 {
            let commands = executor.commands(&restored.state, &mut restored.bots);
            assert_eq!(
                commands,
                executor.commands(&recovered.state, &mut recovered.bots)
            );
            restored.state.tick(&commands);
            recovered.state.tick(&commands);
        }
    }
}

#[test]
fn mixed_cadences_background_and_serial_continue_identically() {
    let mut scenario = Scenario::skirmish();
    for (i, seat) in scenario.players.iter_mut().enumerate() {
        seat.bot = true;
        seat.bot_config = Some(BotConfig::scripted(
            if i == 0 {
                BotDifficulty::Scrapheap
            } else {
                BotDifficulty::Prime
            },
            BotStance::Balanced,
            i as u64,
        ));
    }
    let executor = BotExecutor::new(4);
    let mut state = Arc::new(scenario.build().unwrap());
    let mut bots = seat_bots(&scenario).unwrap();
    bots.reverse();
    let mut expected = bots.clone();
    let mut empty_decisions = 0;
    for _ in 0..180 {
        let job = executor.prepare(&state, &bots, None);
        let due = job.is_some();
        let commands = if let Some(job) = job {
            job.finish(&state, &mut bots, None)
        } else {
            executor.commands(&state, &mut bots)
        };
        if due && commands.is_empty() {
            empty_decisions += 1;
        }
        assert_eq!(commands, serial_commands(&state, &mut expected, None));
        assert_eq!(checkpoint(&bots), checkpoint(&expected));
        Arc::get_mut(&mut state).unwrap().tick(&commands);
    }
    assert!(empty_decisions > 0);
}

#[test]
fn discarded_work_holds_admission_until_finished() {
    let scenario = Scenario::skirmish();
    let executor = BotExecutor::new(2);
    let state = Arc::new(scenario.build().unwrap());
    let bots = seat_bots(&scenario).unwrap();
    let release = block(&executor);
    drop(executor.prepare(&state, &bots, None).unwrap());
    assert!(executor.prepare(&state, &bots, None).is_none());
    release.wait();
    while executor.busy.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    assert_eq!(Arc::strong_count(&state), 1);
    let replacement = Arc::new(scenario.build().unwrap());
    let job = executor.prepare(&replacement, &bots, None).unwrap();
    assert_eq!(
        job.finish(&replacement, &mut bots.clone(), None),
        serial_commands(&state, &mut bots.clone(), None)
    );
}

#[test]
fn unavailable_serial_and_empty_sessions_do_not_dispatch() {
    let scenario = Scenario::skirmish();
    let state = Arc::new(scenario.build().unwrap());
    let bots = seat_bots(&scenario).unwrap();
    assert!(
        BotExecutor::default()
            .prepare(&state, &bots, None)
            .is_none()
    );
    let executor = BotExecutor::new(2);
    assert!(executor.prepare(&state, &[], None).is_none());
    serially(|| assert!(executor.prepare(&state, &bots, None).is_none()));
}

#[test]
fn wrong_world_tick_roster_and_worker_failure_reject_without_installing() {
    let scenario = Scenario::skirmish();
    let state = Arc::new(scenario.build().unwrap());
    let bots = seat_bots(&scenario).unwrap();
    for kind in 0..4 {
        let mut live = bots.clone();
        let before = checkpoint(&live);
        let (send, result) = mpsc::channel();
        let job = PendingDecision {
            world: Arc::downgrade(&state),
            tick: if kind == 1 { 1 } else { 0 },
            result,
        };
        let value = if kind == 3 {
            Err(Box::new("worker failed") as Box<dyn std::any::Any + Send>)
        } else {
            Ok(Decision {
                bots: if kind == 2 { vec![] } else { bots.clone() },
                commands: vec![],
                completed: Instant::now(),
            })
        };
        assert!(send.send(value).is_ok());
        let replacement = Arc::new(scenario.build().unwrap());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            job.finish(
                if kind == 0 { &replacement } else { &state },
                &mut live,
                None,
            )
        }));
        assert!(result.is_err());
        assert_eq!(checkpoint(&live), before);
    }
}

#[test]
fn seven_seats_keep_input_order_and_controller_continuation() {
    let mut scenario = Scenario::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ))
    .unwrap();
    for (i, player) in scenario.players.iter_mut().enumerate() {
        player.bot = i != 0;
        player.bot_config = player.bot.then_some(BotConfig::default());
    }
    let mut bots = seat_bots(&scenario).unwrap();
    assert_eq!(bots.len(), 7);
    bots.reverse();
    let mut expected = bots.clone();
    let mut state = Arc::new(scenario.build().unwrap());
    let executor = BotExecutor::new(4);
    for _ in 0..60 {
        let commands = if let Some(job) = executor.prepare(&state, &bots, None) {
            job.finish(&state, &mut bots, None)
        } else {
            executor.commands(&state, &mut bots)
        };
        assert_eq!(commands, serial_commands(&state, &mut expected, None));
        assert_eq!(checkpoint(&bots), checkpoint(&expected));
        Arc::get_mut(&mut state).unwrap().tick(&commands);
    }
}
