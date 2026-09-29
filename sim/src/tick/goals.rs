//! Where a clicked tile sends each member of a group.
//!
//! A tile goal keeps the tile the player clicked. Each movement domain's half
//! of a group snaps that tile to ground its units can stand on and spreads
//! its members over the open tiles around it, one slot per member in id
//! order, scanned in the group's approach frame. The spread is decided only
//! on ground the owner's team has explored: a click into unexplored ground
//! sends every member to the clicked tile itself, and [`expose`] hands each
//! its slot at the end of the tick that explores the tile. Resolution reads
//! the real map, like every route.

use crate::ids::{PlayerId, UnitId};
use crate::state::{Aim, Goal, Order, State};
use crate::stats::{Domain, GOAL_SNAP_RADIUS};
use chassis::grid::TilePos;
use std::collections::BTreeMap;

/// Radius of the spread around a snapped center.
const SPREAD_RADIUS: i32 = GOAL_SNAP_RADIUS + 3;

/// Every tile within [`SPREAD_RADIUS`]: no spread holds more slots.
pub(super) const MAX_SLOTS: usize = ((2 * SPREAD_RADIUS + 1) * (2 * SPREAD_RADIUS + 1)) as usize;

/// A group member's goal on a clicked tile, decided once for its whole
/// domain half.
pub(super) struct Issue {
    tile: TilePos,
    reverse: bool,
    /// The spread slots, decided now when the owner's team has explored the
    /// tile.
    slots: Option<Vec<TilePos>>,
}

impl Issue {
    /// The goal of the member at `rank`, its place among its domain half in
    /// id order.
    pub(super) fn goal(&self, rank: usize) -> Goal {
        match &self.slots {
            Some(slots) => resolved(self.tile, slots, rank),
            None => Goal::pending(
                self.tile,
                u8::try_from(rank).unwrap_or(u8::MAX),
                self.reverse,
            ),
        }
    }
}

/// `player`'s click on `tile` for its units of `domain`, whose spread scan
/// runs half-turned when `reverse`.
pub(super) fn issue(
    state: &State,
    player: PlayerId,
    tile: TilePos,
    domain: Domain,
    reverse: bool,
) -> Issue {
    let slots = state
        .vision(player)
        .explored(tile)
        .then(|| slots(state, tile, domain, reverse));
    Issue {
        tile,
        reverse,
        slots,
    }
}

/// Every spread slot around `clicked` for `domain` in the frame `reverse`,
/// in rank order and padded with the last to [`MAX_SLOTS`]. Empty when
/// nothing near the clicked tile is open to the domain: the group then
/// heads for the clicked tile and settles as close as it can.
pub(super) fn slots(
    state: &State,
    clicked: TilePos,
    domain: Domain,
    reverse: bool,
) -> Vec<TilePos> {
    group_domain_goal(state, clicked, domain, reverse).map_or_else(Vec::new, |center| {
        spread_goals_by(center, MAX_SLOTS, reverse, |t| {
            state.passable_for(domain, t)
        })
    })
}

/// The goal of the member at `rank` on `clicked` once its slots are known.
fn resolved(clicked: TilePos, slots: &[TilePos], rank: usize) -> Goal {
    match slots.get(rank).or(slots.last()) {
        Some(&slot) => Goal::slotted(clicked, slot),
        None => Goal::at(clicked),
    }
}

/// Hands every pending goal whose clicked tile its owner's team has now
/// explored the slot it was waiting for, in active orders, queued orders,
/// and the marches engagements will resume. The endpoint resolved for the
/// clicked tile goes with it. Runs at the end of the tick, after vision, so
/// no pending goal names an explored tile at a tick boundary.
pub(super) fn expose(state: &mut State) {
    let mut memo: BTreeMap<(TilePos, bool, bool), Vec<TilePos>> = BTreeMap::new();
    for index in 0..state.units.len() {
        let unit = &state.units[index];
        if !std::iter::once(&unit.order)
            .chain(&unit.queue)
            .any(holds_pending)
        {
            continue;
        }
        let (player, domain) = (unit.player, unit.kind.stats().domain);
        let mut order = unit.order;
        let mut queue = std::mem::take(&mut state.units[index].queue);
        {
            let state = &*state;
            let vision = state.vision(player);
            let mut resolve = |goal: &mut Goal| {
                let Aim::Pending { rank, reverse } = goal.aim else {
                    return;
                };
                let tile = goal.tile();
                if !vision.explored(tile) {
                    return;
                }
                let slots = memo
                    .entry((tile, domain == Domain::Air, reverse))
                    .or_insert_with(|| slots(state, tile, domain, reverse));
                *goal = resolved(tile, slots, usize::from(rank));
            };
            for order in std::iter::once(&mut order).chain(queue.iter_mut()) {
                if let Some(goal) = order_goal_mut(order) {
                    resolve(goal);
                }
            }
        }
        let unit = &mut state.units[index];
        unit.order = order;
        unit.queue = queue;
    }
}

/// Whether an order carries a goal still waiting for its slot.
fn holds_pending(order: &Order) -> bool {
    match order {
        Order::Attack { resume, .. } => resume.is_some_and(|goal| goal.is_pending()),
        _ => order.walk_goal().is_some_and(|goal| goal.is_pending()),
    }
}

/// The tile goal an order carries: a walk's own, or the march an
/// engagement resumes.
fn order_goal_mut(order: &mut Order) -> Option<&mut Goal> {
    match order {
        Order::Attack { resume, .. } => resume.as_mut(),
        _ => order.walk_goal_mut(),
    }
}

/// The first tiles `legal` accepts, ring-scanned outward from `center` in
/// the commanded group's approach frame, padded with the last to `count`.
/// Mirrored groups therefore receive mirrored slots instead of inheriting an
/// absolute northwest-first bias.
pub(super) fn spread_goals_by(
    center: TilePos,
    count: usize,
    reverse: bool,
    legal: impl Fn(TilePos) -> bool,
) -> Vec<TilePos> {
    let mut out = Vec::with_capacity(count);
    'scan: for r in 0..=SPREAD_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (dx, dy) = if reverse { (-dx, -dy) } else { (dx, dy) };
                let t = center.offset(dx, dy);
                if legal(t) {
                    out.push(t);
                    if out.len() == count {
                        break 'scan;
                    }
                }
            }
        }
    }
    while out.len() < count {
        out.push(out.last().copied().unwrap_or(center));
    }
    out
}

/// Whether the canonical spread scan needs a half-turn for this group's
/// approach. The first selected unit not already at the goal defines the
/// frame; an owned Foundry is the deterministic fallback for a stacked group.
pub(super) fn spread_scan_reversed(state: &State, center: TilePos, ids: &[UnitId]) -> bool {
    let player = state
        .unit(ids[0])
        .expect("a spread has at least one accepted unit")
        .player;
    let foundry = state
        .buildings
        .iter()
        .find(|building| {
            building.player == player
                && !building.provisional
                && building.kind == crate::stats::BuildingKind::Foundry
        })
        .map(|building| (building.anchor, building.kind.base_stats().size));
    super::group_spread_scan_reversed(
        center,
        ids.iter()
            .map(|id| state.unit(*id).expect("accepted spread unit exists").tile()),
        foundry,
        (state.map.width(), state.map.height()),
        player,
    )
}

/// Resolve a blocked group-command center in the same approach frame used
/// to spread its individual slots.
pub(super) fn group_domain_goal(
    state: &State,
    goal: TilePos,
    domain: Domain,
    reverse: bool,
) -> Option<TilePos> {
    let (center, radius) = match domain {
        Domain::Ground => (goal, GOAL_SNAP_RADIUS),
        Domain::Air => (
            TilePos::new(
                goal.x.clamp(0, state.map.width() - 1),
                goal.y.clamp(0, state.map.height() - 1),
            ),
            GOAL_SNAP_RADIUS + 3,
        ),
    };
    group_goal_by(center, radius, reverse, |t| state.passable_for(domain, t))
}

/// [`group_domain_goal`] over any notion of an open tile.
fn group_goal_by(
    center: TilePos,
    radius: i32,
    reverse: bool,
    legal: impl Fn(TilePos) -> bool,
) -> Option<TilePos> {
    for r in 0..=radius {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (dx, dy) = if reverse { (-dx, -dy) } else { (dx, dy) };
                let candidate = center.offset(dx, dy);
                if legal(candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
    use crate::state::Faction;
    use crate::stats::UnitKind;
    use crate::{Command, PlayerCommand, Scenario};

    /// Rock, peaks and a pit around open ground, small enough for a central
    /// Kestrel to see every tile.
    const FIELD: [&str; 9] = [
        "...........",
        ".##.....^^.",
        ".##.....^^.",
        "....#......",
        "...###.....",
        "....#......",
        "...........",
        ".~~.....##.",
        "...........",
    ];

    fn world(map: &[&str], units: &[(UnitKind, i32, i32)]) -> State {
        Scenario {
            mode: ScenarioMode::Sandbox,
            name: "goals".into(),
            seed: 5,
            map: map.iter().map(|row| (*row).to_owned()).collect(),
            players: vec![PlayerSpec {
                name: "p0".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: units
                .iter()
                .map(|&(kind, x, y)| UnitSpec {
                    player: 0,
                    kind,
                    x,
                    y,
                })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("goals fixture builds")
    }

    fn tiles(state: &State) -> impl Iterator<Item = TilePos> {
        let (w, h) = (state.map.width(), state.map.height());
        (0..h).flat_map(move |y| (0..w).map(move |x| TilePos::new(x, y)))
    }

    /// The tiles a group of `count` took before goals kept the clicked tile:
    /// the snapped center, then the first `count` spread slots around it.
    fn snap_and_spread(
        state: &State,
        clicked: TilePos,
        domain: Domain,
        reverse: bool,
        count: usize,
    ) -> Option<Vec<TilePos>> {
        let center = group_domain_goal(state, clicked, domain, reverse)?;
        Some(spread_goals_by(center, count, reverse, |t| {
            state.passable_for(domain, t)
        }))
    }

    #[test]
    fn explored_slots_are_the_snap_and_spread_tiles_for_every_group_size() {
        let state = world(&FIELD, &[(UnitKind::Kestrel, 5, 4)]);
        for clicked in tiles(&state) {
            for domain in [Domain::Ground, Domain::Air] {
                for reverse in [false, true] {
                    let slots = slots(&state, clicked, domain, reverse);
                    for count in [1, 2, 5, 13, 40, MAX_SLOTS, MAX_SLOTS + 31] {
                        let expected = snap_and_spread(&state, clicked, domain, reverse, count)
                            .expect("premise: every click here has open ground near it");
                        for (rank, tile) in expected.into_iter().enumerate() {
                            let goal = resolved(clicked, &slots, rank);
                            assert_eq!(goal.target(), tile, "{clicked} {domain:?} {rank}");
                            assert_eq!(goal.tile(), clicked);
                            assert!(goal.canonical());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_click_with_no_open_ground_near_it_keeps_the_clicked_tile() {
        let mut map = vec!["#########"; 9];
        map[0] = "........#";
        let state = world(&map, &[(UnitKind::Sentinel, 0, 0)]);
        let clicked = TilePos::new(5, 6);
        assert_eq!(
            group_domain_goal(&state, clicked, Domain::Ground, false),
            None
        );
        assert!(slots(&state, clicked, Domain::Ground, false).is_empty());
        for rank in [0, 3, MAX_SLOTS] {
            assert_eq!(resolved(clicked, &[], rank), Goal::at(clicked));
        }
    }

    #[test]
    fn a_rank_past_the_last_slot_shares_it() {
        let state = world(&FIELD, &[(UnitKind::Kestrel, 5, 4)]);
        let clicked = TilePos::new(5, 4);
        for reverse in [false, true] {
            let slots = slots(&state, clicked, Domain::Ground, reverse);
            assert_eq!(slots.len(), MAX_SLOTS);
            let last = *slots.last().expect("open ground");
            for rank in [MAX_SLOTS - 1, MAX_SLOTS, usize::from(u8::MAX), usize::MAX] {
                assert_eq!(resolved(clicked, &slots, rank).target(), last);
            }
        }
    }

    #[test]
    fn a_group_command_on_explored_ground_takes_its_slots_at_issue() {
        let units = [
            (UnitKind::Sentinel, 0, 0),
            (UnitKind::Kestrel, 10, 8),
            (UnitKind::Sentinel, 0, 8),
            (UnitKind::Kestrel, 5, 8),
            (UnitKind::Sentinel, 6, 0),
            (UnitKind::Sentinel, 10, 0),
        ];
        let mut state = world(&FIELD, &units);
        let clicked = TilePos::new(4, 4);
        assert!(state.vision(PlayerId(0)).explored(clicked));
        let ids: Vec<UnitId> = state.units.iter().map(|u| u.id).collect();
        let command = PlayerCommand {
            player: PlayerId(0),
            command: Command::Run {
                units: ids.clone(),
                goal: clicked,
                queue: false,
            },
        };
        let before = state.clone();
        super::super::commands::apply(&mut state, &[command], &mut Vec::new());
        for domain in [Domain::Ground, Domain::Air] {
            let half: Vec<UnitId> = ids
                .iter()
                .copied()
                .filter(|id| before.unit(*id).unwrap().kind.stats().domain == domain)
                .collect();
            let reverse = spread_scan_reversed(&before, clicked, &half);
            let expected = snap_and_spread(&before, clicked, domain, reverse, half.len())
                .expect("open ground near the click");
            for (id, tile) in half.iter().zip(expected) {
                let Order::Run { goal } = state.unit(*id).unwrap().order else {
                    panic!("unit {id} got a move");
                };
                assert_eq!(goal.tile(), clicked);
                assert_eq!(goal.target(), tile, "unit {id}");
                assert!(!goal.is_pending());
            }
        }
    }

    #[test]
    fn exposure_resolves_every_pending_goal_a_unit_carries() {
        let mut state = world(&FIELD, &[(UnitKind::Sentinel, 5, 6)]);
        let explored = TilePos::new(4, 4);
        let unexplored = TilePos::new(40, 40);
        let reverse = true;
        let slots = slots(&state, explored, Domain::Ground, reverse);
        let unit = &mut state.units[0];
        unit.order = Order::Attack {
            target: crate::AttackTarget::Unit(unit.id),
            pursue: false,
            resume: Some(Goal::pending(explored, 2, reverse)),
        };
        let mut stale = Goal::pending(explored, 4, reverse);
        stale.endpoint = Some(TilePos::new(0, 0));
        unit.queue = [
            Order::Run { goal: stale },
            Order::Hunt {
                goal: Goal::pending(unexplored, 1, false),
            },
            Order::Advance {
                goal: Goal::pending(explored, 200, reverse),
            },
        ]
        .into();
        expose(&mut state);
        let unit = &state.units[0];
        let Order::Attack { resume, .. } = unit.order else {
            panic!("the engagement stands");
        };
        assert_eq!(resume, Some(Goal::slotted(explored, slots[2])));
        assert_eq!(
            unit.queue[0],
            Order::Run {
                goal: Goal::slotted(explored, slots[4])
            },
            "the endpoint resolved for the clicked tile is dropped"
        );
        assert_eq!(
            unit.queue[1],
            Order::Hunt {
                goal: Goal::pending(unexplored, 1, false)
            },
            "an unexplored click keeps waiting"
        );
        assert_eq!(
            unit.queue[2],
            Order::Advance {
                goal: Goal::slotted(explored, slots[MAX_SLOTS - 1])
            }
        );
    }

    #[test]
    fn exposure_leaves_units_without_pending_goals_untouched() {
        let mut state = world(&FIELD, &[(UnitKind::Sentinel, 5, 6)]);
        let mut goal = Goal::slotted(TilePos::new(4, 4), TilePos::new(6, 6));
        goal.endpoint = Some(TilePos::new(6, 7));
        state.units[0].order = Order::Run { goal };
        let before = state.clone();
        expose(&mut state);
        assert_eq!(state, before);
    }
}
