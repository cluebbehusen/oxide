use super::*;

#[test]
fn all_bots_leaves_no_idle_chair() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    assert!(
        scenario.players.iter().any(|p| !p.bot),
        "premise: the shipped map authors a human seat"
    );
    all_bots(&mut scenario);
    assert!(
        scenario
            .players
            .iter()
            .all(|p| p.bot && p.bot_config.is_some()),
        "a scenario bench must field a mind in every chair"
    );
    let mut bots = crate::controller::seat_controllers(&scenario)
        .expect("the valid bench scenario has a public map briefing");
    assert_eq!(
        bots.len(),
        scenario.players.len(),
        "every configured bot seat gets a command source"
    );
    let mut state = scenario.build().expect("skirmish builds");
    let mut issued = 0usize;
    for _ in 0..200 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        issued += commands.len();
        let report = state.tick(&commands);
        crate::controller::record_events(&mut bots, &report);
    }
    assert!(
        issued > 0,
        "the current controller actually plays the benched seats"
    );
}

/// Two identical runs at scale, hash-compared every 50 ticks — the
/// CI face of the bench. Short on purpose: the timed thousands-of-
/// ticks run is the CLI's job on a dev machine.
#[test]
fn five_hundred_units_stay_bit_identical_across_runs() {
    let run = || {
        let scenario = mass_battle(250, 9);
        let mut state = scenario.build().expect("scale scenario builds");
        engage(&mut state);
        let mut hashes = Vec::new();
        for tick in 1..=200u32 {
            state.tick(&[]);
            if tick % 50 == 0 {
                hashes.push(state.hash());
            }
        }
        hashes
    };
    assert_eq!(run(), run(), "scale must not cost determinism");
}
