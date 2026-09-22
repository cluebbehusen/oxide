//! Bot behavior exercised through the ordinary simulation.

mod common;
use common::standard_brain;
use oxide_sim::{Event, PlayerId, Scenario, UnitKind};

#[test]
fn bot_economy_progresses_against_an_idle_opponent() {
    // The exact configuration that froze pre-fix: shipped skirmish, human
    // seat idle, Standard Cupric bot alone. Its bank plus spending must exceed its
    // starting stake — deposits happened — well before 12k ticks.
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    assert!(
        scenario.players[1].bot,
        "skirmish ships with exactly one bot seat"
    );
    let mut bot = standard_brain(&scenario, PlayerId(1));

    let mut deposited = 0u32;
    for _ in 0..12_000u64 {
        let commands = bot.act(&state);
        let report = state.tick(&commands);
        for event in &report.events {
            if let Event::ScrapDeposited {
                player: PlayerId(1),
                amount,
            } = event
            {
                deposited += amount;
            }
        }
        if deposited >= 300 {
            return; // healthy economy, no need to run the rest
        }
    }
    panic!("bot deposited only {deposited} scrap in 12k ticks — economy stalled");
}

#[test]
fn bot_reaches_its_tech_and_mixes_its_army() {
    use oxide_sim::stats::BuildingKind;
    // A funded bank makes technology affordable while the bot still has to
    // construct and use its producer through ordinary commands.
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot = true;
    scenario.players[1].scrap = 3500;
    let mut state = scenario.build().unwrap();
    let mut bot = standard_brain(&scenario, PlayerId(1));
    let mut trained_advanced = false;
    let mut completed_fabricator = false;
    for _ in 0..12_000u32 {
        if state.result().is_some() {
            break;
        }
        let commands = bot.act(&state);
        let report = state.tick(&commands);
        completed_fabricator |= report.events.iter().any(|event| {
            matches!(
                event,
                Event::BuildingCompleted {
                    player: PlayerId(1),
                    kind: BuildingKind::Fabricator,
                    ..
                }
            )
        });
        trained_advanced |= report.events.iter().any(|event| {
            matches!(
                event,
                Event::UnitTrained {
                    player: PlayerId(1),
                    kind: UnitKind::Lancer | UnitKind::Warden,
                    ..
                }
            )
        });
    }
    assert!(
        completed_fabricator,
        "no completed Fabricator within 12k ticks"
    );
    assert!(
        trained_advanced,
        "Fabricator never completed an advanced unit within 12k ticks"
    );
}
