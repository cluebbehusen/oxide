//! Bot behavior exercised through the ordinary simulation.

use oxide_sim::Scenario;

#[test]
fn a_2v2_scenario_reproduces_bit_identically() {
    let scenario = Scenario::load("../scenarios/twin-forges.json").unwrap();
    let run = || {
        let mut state = scenario.build().unwrap();
        let mut bots = oxide_bot::seat_bots(&scenario).unwrap();
        assert!(!bots.is_empty(), "twin-forges fields bot seats");
        for _ in 0..600 {
            let mut commands = Vec::new();
            for bot in bots.iter_mut() {
                commands.extend(bot.act(&state));
            }
            state.tick(&commands);
        }
        state.hash()
    };
    assert_eq!(run(), run(), "same seed, same commands, same world");
}
