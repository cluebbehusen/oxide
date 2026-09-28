//! The orders dock's chips remove the order they show from every selected
//! unit that still has it, reading the program the staged commands leave.

use super::*;
use crate::panel::{CardAction, CardIcon, OrderSubject};
use macroquad::math::Rect;
use oxide_sim::{OrderKey, PlayerId, State, UnitId};

/// Publishes the selection's orders dock inside the panel band, as drawing
/// does, and returns each chip's rect.
fn publish_dock(game: &mut Game, input: &InputState) -> Vec<Rect> {
    let panel = crate::panel::build_for_input(&game.view(), input).expect("a panel");
    let mut layout = bare_layout(680.0, 900.0);
    let rects: Vec<Rect> = (0..panel.queue.len())
        .map(|slot| Rect::new(20.0 + 52.0 * slot as f32, 700.0, 48.0, 48.0))
        .collect();
    for (slot, (rect, chip)) in rects.iter().zip(&panel.queue).enumerate() {
        layout.queue_slots[slot] = (*rect, chip.action);
    }
    layout.queue_count = rects.len();
    game.presentation.layout.set(layout);
    *game.presentation.panel_model.borrow_mut() = Some(panel);
    rects
}

/// The dock's walk chips: each one's clicked tile and later repeats.
fn dock_walks(game: &Game, input: &InputState) -> Vec<(TilePos, u8)> {
    crate::panel::build_for_input(&game.view(), input)
        .expect("a panel")
        .queue
        .iter()
        .map(|chip| match chip.action {
            CardAction::CancelOrder {
                key: OrderKey::Walk { tile },
                from_end,
                ..
            } => (tile, from_end),
            other => panic!("a walk chip, not {other:?}"),
        })
        .collect()
}

/// A unit's walks in the world, by clicked tile.
fn walks(state: &State, id: UnitId) -> Vec<TilePos> {
    let unit = state.unit(id).expect("the walker lives");
    std::iter::once(&unit.order)
        .chain(&unit.queue)
        .map(|order| match order.key(state, unit.player) {
            Some(OrderKey::Walk { tile }) => tile,
            other => panic!("a walk, not {other:?}"),
        })
        .collect()
}

fn legs(player: PlayerId, unit: UnitId, goals: &[TilePos]) -> Vec<PlayerCommand> {
    goals
        .iter()
        .enumerate()
        .map(|(leg, &goal)| PlayerCommand {
            player,
            command: Command::Move {
                units: vec![unit],
                goal,
                queue: leg > 0,
            },
        })
        .collect()
}

fn press(game: &mut Game, input: &mut InputState, chip: Rect) {
    apply_events(game, input, &click(chip.center().x, chip.center().y));
}

#[test]
fn paused_chip_clicks_remove_the_legs_they_show() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let human = game.presentation.human;
    let (fighter, _) = own_fighter(&game);
    game.presentation.selection.units = vec![fighter];
    let [a, b, c, d] = [(12, 2), (16, 2), (20, 2), (24, 2)].map(|(x, y)| TilePos::new(x, y));
    game.state.tick(&legs(human, fighter, &[a, b, c, d]));
    game.presentation.paused = true;
    assert_eq!(dock_walks(&game, &input), [(a, 0), (b, 0), (c, 0), (d, 0)]);

    let chips = publish_dock(&mut game, &input);
    press(&mut game, &mut input, chips[1]);
    assert_eq!(
        dock_walks(&game, &input),
        [(a, 0), (c, 0), (d, 0)],
        "the dock shows the program the staged removal leaves"
    );
    let chips = publish_dock(&mut game, &input);
    press(&mut game, &mut input, chips[2]);
    assert_eq!(dock_walks(&game, &input), [(a, 0), (c, 0)]);

    let at = |tile: TilePos| {
        game.presentation
            .camera
            .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5))
    };
    let unit = game.state.unit(fighter).unwrap();
    let points: Vec<_> = crumbs(&game, unit)
        .into_iter()
        .map(|(index, point, _)| (index, point))
        .collect();
    assert_eq!(
        points,
        [(0, at(a)), (1, at(c))],
        "the waypoints renumber with the dock"
    );
    assert_eq!(walks(&game.state, fighter), [a, b, c, d], "paused");

    game.advance_ticks(1);
    assert_eq!(walks(&game.state, fighter), [a, c]);
}

/// Open ground for a Turret near `worker`.
fn turret_ground(game: &Game, worker: UnitId) -> TilePos {
    let here = game.state.unit(worker).unwrap().tile();
    let player = game.state.unit(worker).unwrap().player;
    (-6..=6)
        .flat_map(|dy| (-6..=6).map(move |dx| here.offset(dx, dy)))
        .find(|&anchor| {
            game.state
                .can_place(player, oxide_sim::BuildingKind::Turret, anchor)
        })
        .expect("open ground near the harvester")
}

#[test]
fn a_staged_sites_chip_cancels_it_by_kind_and_anchor_whoever_takes_its_id() {
    use crate::game::network::NetRole;
    use oxide_sim::BuildingKind::Turret;
    let (host_seat, client_seat) = (PlayerId(0), PlayerId(1));
    let mut duel = oxide_sim::Scenario::skirmish();
    for player in &mut duel.players {
        player.bot = false;
        player.bot_config = None;
        player.scrap = 5_000;
    }
    let viewport = vec2(1280.0, 800.0);
    let mut host = Game::networked(duel.clone(), host_seat, NetRole::Host, viewport).unwrap();
    let mut client = Game::networked(duel, client_seat, NetRole::Client, viewport).unwrap();
    let mut input = InputState::new();
    let harvester = |game: &Game, seat| {
        game.state
            .units()
            .iter()
            .find(|u| u.player == seat && u.kind == UnitKind::Harvester)
            .expect("a starting harvester")
            .id
    };
    let (worker, rival) = (harvester(&client, client_seat), harvester(&host, host_seat));
    let (anchor, rival_anchor) = (turret_ground(&client, worker), turret_ground(&host, rival));
    let build = |units: Vec<UnitId>, anchor| Command::Build {
        units,
        kind: Turret,
        anchor,
        queue: false,
        defer: false,
    };

    client.issue(build(vec![worker], anchor));
    client.presentation.selection.units = vec![worker];
    let projected = client.state.inspect_command_phase(&client.pending, |view| {
        view.buildings()
            .iter()
            .find(|b| b.anchor == anchor)
            .expect("the staged site")
            .id
    });
    assert!(client.state.building(projected).is_none(), "premise");
    let panel = crate::panel::build_for_input(&client.view(), &input).expect("a panel");
    assert_eq!(
        panel.queue[0].action,
        CardAction::CancelFound(Turret, anchor)
    );
    assert!(matches!(
        panel.queue[0].icon,
        CardIcon::Order {
            subject: OrderSubject::Building(Turret, _),
            ..
        }
    ));
    let chips = publish_dock(&mut client, &input);
    press(&mut client, &mut input, chips[0]);

    // The host's own site lands first in the batch and takes the id the
    // client's site was projected with.
    let mut batch = vec![PlayerCommand {
        player: host_seat,
        command: build(vec![rival], rival_anchor),
    }];
    batch.extend(
        client
            .take_outbox()
            .into_iter()
            .map(|command| PlayerCommand {
                player: client_seat,
                command,
            }),
    );
    assert_eq!(batch.len(), 3, "the host's site, the client's, its removal");
    let bank = client.state.player(client_seat).scrap;
    for game in [&mut host, &mut client] {
        game.run_batch(&batch);
    }
    let taken = client.state.building(projected).expect("the host's site");
    assert_eq!(
        (taken.player, taken.anchor),
        (host_seat, rival_anchor),
        "premise: another seat took the projected id"
    );
    assert!(client.state.buildings().iter().all(|b| b.anchor != anchor));
    assert_eq!(client.state.player(client_seat).scrap, bank, "refunded");
    assert_eq!(client.state.hash(), host.state.hash());
}

#[test]
fn a_chip_answers_a_slow_tap_but_not_a_lift_after_a_hold() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let human = game.presentation.human;
    let (fighter, _) = own_fighter(&game);
    game.presentation.selection.units = vec![fighter];
    let [a, b] = [(12, 2), (16, 2)].map(|(x, y)| TilePos::new(x, y));
    game.state.tick(&legs(human, fighter, &[a, b]));
    game.presentation.paused = true;
    let chips = publish_dock(&mut game, &input);
    let p = chips[1].center();
    let hold = f64::from(input.touch_prefs.long_press_ms) / 1000.0;

    input.now = 5.0;
    apply_events(&mut game, &mut input, &[touch_down(1, p)]);
    input.now += hold + 0.05;
    assert!(
        input.touch_preview().is_some(),
        "premise: the held chip previews"
    );
    apply_events(&mut game, &mut input, &[touch_up(1, p)]);
    assert!(
        game.pending.is_empty(),
        "a hold that read the chip removes nothing"
    );

    input.now += 1.0;
    apply_events(&mut game, &mut input, &[touch_down(2, p)]);
    input.now += 0.2;
    assert!(0.2 < hold, "premise: a slow tap, not a hold");
    apply_events(&mut game, &mut input, &[touch_up(2, p)]);
    assert!(matches!(
        game.pending.as_slice(),
        [PlayerCommand {
            command: Command::CancelOrder {
                key: OrderKey::Walk { tile },
                from_end: 0,
                ..
            },
            ..
        }] if *tile == b
    ));
    assert_eq!(dock_walks(&game, &input), [(a, 0)]);
}

/// Holds the dock's first chip past the long-press threshold and lifts,
/// then taps it: how many commands the hold staged, and what the tap did.
fn hold_then_tap(game: &mut Game, input: &mut InputState) -> (usize, Vec<Command>) {
    let chips = publish_dock(game, input);
    let p = chips[0].center();
    let hold = f64::from(input.touch_prefs.long_press_ms) / 1000.0;
    input.now = 5.0;
    apply_events(game, input, &[touch_down(1, p)]);
    input.now += hold + 0.05;
    apply_events(game, input, &[touch_up(1, p)]);
    let held = game.pending.len();
    input.now += 1.0;
    apply_events(game, input, &[touch_down(2, p)]);
    input.now += 0.1;
    apply_events(game, input, &[touch_up(2, p)]);
    let tapped = game.pending.iter().map(|c| c.command.clone()).collect();
    (held, tapped)
}

#[test]
fn a_held_site_chip_scraps_nothing_but_a_tap_cancels_the_site() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let human = game.presentation.human;
    let worker = game
        .state
        .units()
        .iter()
        .find(|u| u.player == human && u.kind == UnitKind::Harvester)
        .expect("a starting harvester")
        .id;
    let anchor = turret_ground(&game, worker);
    game.state.tick(&[PlayerCommand {
        player: human,
        command: Command::Build {
            units: vec![worker],
            kind: oxide_sim::BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
        },
    }]);
    let site = game
        .state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor && !b.built)
        .expect("the site stands")
        .id;
    game.presentation.selection.units = vec![worker];
    game.presentation.paused = true;

    let (held, tapped) = hold_then_tap(&mut game, &mut input);

    assert_eq!(held, 0, "a hold that read the site chip scraps nothing");
    assert!(
        matches!(tapped.as_slice(), [Command::Cancel { building }] if *building == site),
        "a tap still cancels the site: {tapped:?}"
    );
}

#[test]
fn a_held_production_chip_cancels_nothing_but_a_tap_does() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let foundry = game.home_foundry().expect("a Foundry").id;
    game.presentation.selection.buildings = vec![foundry];
    super::super::orders::train(&mut game, 0);
    let staged: Vec<PlayerCommand> = game.pending.drain(..).collect();
    assert_eq!(staged.len(), 1, "premise: one job queued");
    game.state.tick(&staged);
    game.presentation.paused = true;

    let (held, tapped) = hold_then_tap(&mut game, &mut input);

    assert_eq!(
        held, 0,
        "a hold that read the production chip cancels nothing"
    );
    assert!(
        matches!(
            tapped.as_slice(),
            [Command::CancelTrain { building, index: 0 }] if *building == foundry
        ),
        "a tap still cancels the job: {tapped:?}"
    );
}

#[test]
fn a_chip_sends_the_rest_of_the_selection_along() {
    let mut game = headless_game();
    let mut input = InputState::new();
    let human = game.presentation.human;
    let mine: Vec<UnitId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == human)
        .map(|u| u.id)
        .collect();
    let [a, b] = [(12, 2), (16, 2)].map(|(x, y)| TilePos::new(x, y));
    for &unit in &mine {
        game.state.tick(&legs(human, unit, &[a, b]));
    }
    game.presentation.selection.units = mine.clone();
    let subject = crate::panel::subject_unit(&game.view()).expect("a subject");
    let chips = publish_dock(&mut game, &input);
    press(&mut game, &mut input, chips[0]);
    let [
        PlayerCommand {
            command: Command::CancelOrder { unit, units, .. },
            ..
        },
    ] = game.pending.as_slice()
    else {
        panic!("one removal staged: {:?}", game.pending);
    };
    assert_eq!(*unit, subject);
    let mut everyone = units.clone();
    everyone.push(subject);
    everyone.sort_unstable();
    assert_eq!(everyone, mine, "the subject names itself once");
    game.advance_ticks(1);
    for id in mine {
        assert_eq!(walks(&game.state, id), [b], "{id} dropped the leg");
    }
}

#[test]
fn a_lan_clients_chip_names_the_leg_it_showed_after_legs_finish_in_flight() {
    use crate::game::network::NetRole;
    let (host_seat, client_seat) = (PlayerId(0), PlayerId(1));
    let mut duel = oxide_sim::Scenario::skirmish();
    for player in &mut duel.players {
        player.bot = false;
        player.bot_config = None;
    }
    let viewport = vec2(1280.0, 800.0);
    let mut host = Game::networked(duel.clone(), host_seat, NetRole::Host, viewport).unwrap();
    let mut client = Game::networked(duel, client_seat, NetRole::Client, viewport).unwrap();
    let mut input = InputState::new();
    let own = |kind| {
        client
            .state
            .units()
            .iter()
            .find(|u| u.player == client_seat && u.kind == kind)
            .expect("the client seat's starting unit")
            .id
    };
    let (walker, patroller) = (own(UnitKind::Sentinel), own(UnitKind::Harvester));
    let tile = |(x, y)| TilePos::new(x, y);
    let [near, far, farther] = [(30, 14), (20, 20), (10, 20)].map(tile);
    let [lap, out, back] = [(32, 15), (20, 21), (10, 21)].map(tile);
    let sent = |game: &mut Game| -> Vec<PlayerCommand> {
        game.take_outbox()
            .into_iter()
            .map(|command| PlayerCommand {
                player: client_seat,
                command,
            })
            .collect()
    };

    for order in legs(client_seat, walker, &[near, far, farther]) {
        client.issue(order.command);
    }
    client.issue(Command::Patrol {
        units: vec![patroller],
        waypoints: vec![lap, out, back],
    });
    let programs = sent(&mut client);
    for game in [&mut host, &mut client] {
        game.run_batch(&programs);
    }

    client.presentation.selection.units = vec![walker];
    let chips = publish_dock(&mut client, &input);
    press(&mut client, &mut input, chips[1]);
    client.presentation.selection.units = vec![patroller];
    let chips = publish_dock(&mut client, &input);
    press(&mut client, &mut input, chips[2]);
    let removals = sent(&mut client);
    assert_eq!(removals.len(), 2);

    // The walker finishes its first leg, and the patrol rotates its first
    // leg to the back, while the removals are on their way.
    let in_flight = |game: &Game| {
        walks(&game.state, walker) == [far, farther]
            && walks(&game.state, patroller) == [out, back, lap]
    };
    for _ in 0..200 {
        if in_flight(&client) {
            break;
        }
        for game in [&mut host, &mut client] {
            game.run_batch(&[]);
        }
    }
    assert!(in_flight(&client), "premise: legs finished in flight");
    assert_eq!(client.pending.len(), 2, "both removals still projected");

    for game in [&mut host, &mut client] {
        game.run_batch(&removals);
    }
    assert_eq!(walks(&client.state, walker), [farther]);
    assert_eq!(walks(&client.state, patroller), [out, lap]);
    assert!(client.pending.is_empty());
    assert_eq!(client.state.hash(), host.state.hash());
}
