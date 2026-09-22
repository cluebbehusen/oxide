//! Bot behavior exercised through the ordinary simulation.

mod common;
use chassis::grid::TilePos;
use common::simulation::*;
use common::standard_brain;
use oxide_sim::{Command, PlayerId, UnitKind};

#[test]
fn bot_sends_a_relief_builder_to_an_orphaned_site() {
    use oxide_sim::stats::BuildingKind;
    // Manufacture the orphan directly: a scripted Build, then Stop the
    // builder on the spot. Hand the seat to a bot — its relief loop must
    // finish the paid-for site (a pending site once suppressed fabricator
    // logic forever instead).
    let scenario = arena(vec![
        unit(1, UnitKind::Harvester, 12, 2),
        unit(1, UnitKind::Harvester, 13, 3),
    ]);
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(12, 1);
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![builder],
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    assert!(!state.building(site).unwrap().built);

    let mut bot = standard_brain(&scenario, PlayerId(1));
    for _ in 0..3000u32 {
        let commands = bot.act(&state);
        state.tick(&commands);
        if state.building(site).is_some_and(|b| b.built) {
            return;
        }
    }
    panic!("orphaned site was never resumed by the bot");
}
