use super::*;
use chassis::grid::TilePos;
use oxide_opponent::MissionKind;
use oxide_sim::{BuildingKind, PlayerId, Scenario};

fn launch(mission: u64, defense: u64, sent: u64) -> Launch {
    Launch {
        mission,
        kind: kind(),
        defense,
        margin: 1_500,
        need: defense * 3 / 2,
        sent,
        units: Vec::new(),
    }
}

fn kind() -> MissionKind {
    MissionKind::Attack {
        owner: PlayerId(1),
        building: BuildingKind::Foundry,
        anchor: TilePos::new(20, 2),
    }
}

fn status(id: u64, phase: Phase) -> MissionStatus {
    MissionStatus {
        id,
        kind: kind(),
        phase,
        since: 0,
        timeout: 1_200,
        units: 4,
        goal: TilePos::new(10, 2),
    }
}

#[test]
fn attacks_are_followed_to_their_end_and_grouped_by_what_was_sent() {
    let state = Scenario::skirmish().build().unwrap();
    let ledger = ImpactLedger::new(&state);
    let mut follower = AttackFollower::new([true, false]);
    let launches = [launch(1, 0, 600), launch(2, 400, 700), launch(3, 100, 900)];
    follower.open(0, &launches, &ledger);
    follower.follow(
        0,
        12,
        &[
            status(1, Phase::Gather),
            status(2, Phase::Gather),
            status(3, Phase::Gather),
        ],
        &ledger,
    );
    follower.open(0, &launches, &ledger);
    follower.follow(
        0,
        24,
        &[status(2, Phase::Engage), status(3, Phase::Engage)],
        &ledger,
    );
    follower.follow(0, 36, &[status(2, Phase::Withdraw)], &ledger);
    follower.open(1, &launches, &ledger);
    follower.follow(1, 36, &[], &ledger);
    let [Some(seat), None] = &follower.finish(&ledger)[..] else {
        panic!("only the opponent seat is followed");
    };
    assert_eq!(
        (seat.unknown.attacks, seat.unknown.no_contact),
        (1, 1),
        "attack 1 ended before it fought"
    );
    assert_eq!(
        (seat.under_2x.attacks, seat.under_2x.withdrew),
        (1, 1),
        "attack 2 withdrew"
    );
    assert_eq!(
        (seat.over_4x.attacks, seat.over_4x.fought),
        (1, 1),
        "attack 3 fought to its end"
    );
    assert_eq!(seat.from_2x_to_4x, AttackBucket::default());
    let mut rendered = String::new();
    seat.render(&mut rendered, "");
    assert!(rendered.contains("under 2x"), "{rendered}");
    assert!(!rendered.contains("2x to 4x"), "{rendered}");
}

#[test]
fn an_ended_attack_closes_when_its_units_launch_again() {
    let state = Scenario::skirmish().build().unwrap();
    let ledger = ImpactLedger::new(&state);
    let mut follower = AttackFollower::new([true]);
    let unit = |id| oxide_sim::UnitId(id);
    let first = Launch {
        units: vec![unit(7), unit(8)],
        ..launch(1, 0, 600)
    };
    follower.open(0, std::slice::from_ref(&first), &ledger);
    follower.follow(0, 12, &[], &ledger);
    let open = |follower: &AttackFollower| -> Vec<u64> {
        follower.seats[0]
            .as_ref()
            .unwrap()
            .open
            .keys()
            .copied()
            .collect()
    };
    assert_eq!(open(&follower), [1], "ended, still in its tail");
    let second = Launch {
        units: vec![unit(8), unit(9)],
        ..launch(2, 0, 600)
    };
    follower.open(0, &[first, second], &ledger);
    assert_eq!(
        open(&follower),
        [2],
        "the first closed when unit 8 relaunched"
    );
    let [Some(seat)] = &follower.finish(&ledger)[..] else {
        panic!();
    };
    assert_eq!(seat.unknown.attacks, 2);
}
