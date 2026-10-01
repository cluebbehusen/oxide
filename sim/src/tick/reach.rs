//! Brain-phase reachability: which ground or sky a unit can reach, the
//! nearest tile it can settle for when its goal lies beyond, and the parked
//! crowds that end such a walk early.
//!
//! Everything here is scratch owned by one brain phase and rebuilt lazily.
//! Nothing reaches [`State`]; the answers depend only on the world the phase
//! started from plus the passability writes the owner reports.

use super::spatial::UnitIndex;
use crate::ids::{PlayerId, UnitId};
use crate::state::{Order, State};
use crate::stats::Domain;
use chassis::fx::Fx;
use chassis::grid::{CARDINALS, TilePos};
use std::collections::BTreeMap;

/// Up to four component labels a unit can route into, ascending and padded
/// with zeros. An empty set means the unit is sealed in.
type LabelSet = [u32; 4];

/// Reachability answers for one brain phase.
///
/// Component labelings match [`chassis::path::astar`] reachability under the
/// same passability the routes use, so a target outside the unit's labels is
/// one no route can reach. Labels are built on first use and describe
/// passability at that moment. The only passability write inside the brain
/// phase is a scrap node running dry, which a harvester reports with
/// [`crate::Event::NodeDepleted`]; the owner must call
/// [`Reach::forget_ground`] after any brain that emitted one. Sky
/// passability never changes during a match.
///
/// Settled crowds are judged against the bodies that stood idle, pathless,
/// and stopped when the phase began, so the answer never depends on which
/// brain asked first.
pub(super) struct Reach {
    ground: Option<Vec<u32>>,
    air: Option<Vec<u32>>,
    endpoints: BTreeMap<(u8, LabelSet, TilePos, bool), Option<TilePos>>,
    /// Crowd chains by movement layer, the asking walker's team, and
    /// endpoint.
    crowds: BTreeMap<(u8, u8, TilePos), Vec<usize>>,
    /// Movement layer and owner of each unit slot that is parked for crowd
    /// purposes.
    parked: Vec<Option<(Domain, PlayerId)>>,
}

fn domain_key(domain: Domain) -> u8 {
    match domain {
        Domain::Ground => 0,
        Domain::Air => 1,
    }
}

impl Reach {
    /// Scratch for a brain phase starting from `state`.
    pub(super) fn new(state: &State) -> Self {
        let parked = state
            .units
            .iter()
            .map(|unit| {
                (unit.hp > 0
                    && unit.path.is_none()
                    && unit.drive_speed == Fx::ZERO
                    && unit.order == Order::Idle)
                    .then(|| (unit.domain(), unit.player))
            })
            .collect();
        Self {
            ground: None,
            air: None,
            endpoints: BTreeMap::new(),
            crowds: BTreeMap::new(),
            parked,
        }
    }

    /// Drops everything derived from ground passability after a scrap node
    /// opened its tile.
    pub(super) fn forget_ground(&mut self) {
        self.ground = None;
        self.endpoints
            .retain(|&(domain, ..), _| domain != domain_key(Domain::Ground));
    }

    /// The tile unit `id` should route to for `target`: the target itself
    /// when it can be reached, otherwise the reachable tile nearest it.
    /// `None` means the unit cannot move anywhere.
    ///
    /// Among equally near tiles the ring scan runs in the spread-slot order,
    /// half-turned by the frame between the target and the unit, so mirrored
    /// units settle on mirrored tiles. `stored` is the endpoint the order
    /// already holds; it wins a tie so a recompute never hops the unit to an
    /// equally near partner tile.
    pub(super) fn endpoint(
        &mut self,
        state: &State,
        id: UnitId,
        target: TilePos,
        stored: Option<TilePos>,
    ) -> Option<TilePos> {
        let unit = state.unit(id)?;
        let domain = unit.kind.stats().domain;
        let from = unit.tile();
        let player = unit.player;
        let labels = labels(
            match domain {
                Domain::Ground => &mut self.ground,
                Domain::Air => &mut self.air,
            },
            state,
            domain,
        );
        let width = state.map.width();
        let height = state.map.height();
        let label = |tile: TilePos| label_at(labels, width, height, tile);
        // A* refuses a start off the map, so a unit standing there is sealed.
        let on_map = from.x >= 0 && from.y >= 0 && from.x < width && from.y < height;
        let set = if on_map {
            label_set(&label, from)
        } else {
            [0; 4]
        };
        if set[0] == 0 {
            return None;
        }
        let member = |tile: TilePos| {
            let l = label(tile);
            l != 0 && set.contains(&l)
        };
        if member(target) {
            return Some(target);
        }
        let center = TilePos::new(
            target.x.clamp(0, width.max(1) - 1),
            target.y.clamp(0, height.max(1) - 1),
        );
        let reverse = scan_reversed(state, player, center, from);
        let key = (domain_key(domain), set, center, reverse);
        let best = match self.endpoints.get(&key) {
            Some(&best) => best,
            None => {
                let best = nearest(center, reverse, width.max(height), member);
                self.endpoints.insert(key, best);
                best
            }
        };
        let best = best?;
        if let Some(stored) = stored
            && member(stored)
            && distance_sq(stored, center) == distance_sq(best, center)
        {
            return Some(stored);
        }
        Some(best)
    }

    /// Whether unit `id` touches a parked body of its own layer that
    /// connects, through touching parked bodies, to one within
    /// [`crate::stats::ARRIVAL_NEAR`] of `endpoint`, counting only bodies
    /// its side may count. A short walk that meets the crowd already
    /// standing at its endpoint has gone as far as it can.
    pub(super) fn crowd_touches(
        &mut self,
        state: &State,
        index: &UnitIndex,
        id: UnitId,
        endpoint: TilePos,
    ) -> bool {
        let Some(unit) = state.unit(id) else {
            return false;
        };
        // Every chain body lies within CROWD_CHAIN_REACH of the endpoint and
        // any touch spans less than CONTACT_WINDOW.
        let span = crate::stats::CROWD_CHAIN_REACH + Fx::from_num(CONTACT_WINDOW);
        if unit.pos.dist_sq(endpoint.center()) > span * span {
            return false;
        }
        let domain = unit.domain();
        let team = state.player(unit.player).team;
        let key = (domain_key(domain), team, endpoint);
        if !self.crowds.contains_key(&key) {
            let chain = self.crowd(state, index, domain, unit.player, endpoint);
            self.crowds.insert(key, chain);
        }
        let chain = &self.crowds[&key];
        if chain.is_empty() {
            return false;
        }
        let tile = unit.tile();
        (tile.y - CONTACT_WINDOW..=tile.y + CONTACT_WINDOW).any(|y| {
            index
                .row_span(y, tile.x - CONTACT_WINDOW, tile.x + CONTACT_WINDOW)
                .iter()
                .any(|&(_, slot)| {
                    let other = &state.units[slot];
                    other.id != id && chain.binary_search(&slot).is_ok() && touching(unit, other)
                })
        })
    }

    /// The parked bodies of `domain` connected to the arrival disk around
    /// `endpoint`, as sorted unit slots, as `player`'s side may count them.
    /// Membership is a set, so the visit order cannot change it.
    ///
    /// Only bodies not hostile to `player` count, whether they seed the
    /// chain inside the disk or link it beyond: a hostile body may stand
    /// outside the side's sight, and counting it would reveal where it
    /// stands and whether it is idle.
    fn crowd(
        &self,
        state: &State,
        index: &UnitIndex,
        domain: Domain,
        player: PlayerId,
        endpoint: TilePos,
    ) -> Vec<usize> {
        let center = endpoint.center();
        let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
        let bound_sq = crate::stats::CROWD_CHAIN_REACH * crate::stats::CROWD_CHAIN_REACH;
        let countable = |slot: usize, within_sq: Fx| {
            self.parked
                .get(slot)
                .copied()
                .flatten()
                .is_some_and(|(layer, owner)| layer == domain && !state.hostile(player, owner))
                && state.units[slot].pos.dist_sq(center) <= within_sq
        };
        let mut chain = Vec::new();
        let mut frontier = Vec::new();
        let seed = crate::stats::ARRIVAL_NEAR.to_num::<i32>() + 1;
        for y in endpoint.y - seed..=endpoint.y + seed {
            for &(_, slot) in index.row_span(y, endpoint.x - seed, endpoint.x + seed) {
                if countable(slot, near_sq) {
                    chain.push(slot);
                    frontier.push(slot);
                }
            }
        }
        chain.sort_unstable();
        while let Some(slot) = frontier.pop() {
            let body = &state.units[slot];
            let tile = body.tile();
            for y in tile.y - CONTACT_WINDOW..=tile.y + CONTACT_WINDOW {
                for &(_, other) in
                    index.row_span(y, tile.x - CONTACT_WINDOW, tile.x + CONTACT_WINDOW)
                {
                    if !countable(other, bound_sq) || !touching(body, &state.units[other]) {
                        continue;
                    }
                    if let Err(at) = chain.binary_search(&other) {
                        chain.insert(at, other);
                        frontier.push(other);
                    }
                }
            }
        }
        chain
    }
}

/// One layer's component labels, built on first use.
fn labels<'a>(slot: &'a mut Option<Vec<u32>>, state: &State, domain: Domain) -> &'a [u32] {
    slot.get_or_insert_with(|| {
        chassis::path::cardinal_components(state.map.width(), state.map.height(), |tile| {
            state.passable_for(domain, tile)
        })
    })
}

/// Tiles either side of a body's own tile that can hold a body touching it:
/// the widest pair of radii plus contact slack stays under two tiles.
const CONTACT_WINDOW: i32 = 2;

/// Whether two bodies are in contact, with the slack the arrival wave uses.
fn touching(a: &crate::state::Unit, b: &crate::state::Unit) -> bool {
    let slack = const { Fx::lit("0.05") };
    a.pos.dist(b.pos) <= a.kind.stats().radius + b.kind.stats().radius + slack
}

/// A tile's component label; off-map tiles read as closed.
fn label_at(labels: &[u32], width: i32, height: i32, tile: TilePos) -> u32 {
    if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
        return 0;
    }
    labels
        .get((tile.y as usize) * (width as usize) + tile.x as usize)
        .copied()
        .unwrap_or(0)
}

/// The components a route from `from` can enter: its own, or, when it stands
/// on a closed tile, those of its open cardinal neighbours. A* never cuts a
/// corner, so a diagonal neighbour alone opens nothing.
fn label_set(label: &impl Fn(TilePos) -> u32, from: TilePos) -> LabelSet {
    let mut set = [0; 4];
    let own = label(from);
    if own != 0 {
        set[0] = own;
        return set;
    }
    let mut len = 0;
    for (dx, dy) in CARDINALS {
        let l = label(from.offset(dx, dy));
        if l != 0 && !set[..len].contains(&l) {
            set[len] = l;
            len += 1;
        }
    }
    set[..len].sort_unstable();
    set
}

/// The half-turn frame for a unit's endpoint scan: the unit's approach to
/// the target, falling back to its owner's first Foundry when it stands on
/// the target itself.
fn scan_reversed(
    state: &State,
    player: crate::ids::PlayerId,
    center: TilePos,
    from: TilePos,
) -> bool {
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
        [from],
        foundry,
        (state.map.width(), state.map.height()),
        player,
    )
}

fn distance_sq(a: TilePos, b: TilePos) -> i64 {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dy = i64::from(a.y) - i64::from(b.y);
    dx * dx + dy * dy
}

/// The accepted tile nearest `center` by squared distance, ties to the
/// earliest in the spread-slot ring order. Rings grow until no tile in the
/// next one could be strictly nearer.
fn nearest(
    center: TilePos,
    reverse: bool,
    max_radius: i32,
    accept: impl Fn(TilePos) -> bool,
) -> Option<TilePos> {
    let mut best: Option<(i64, TilePos)> = None;
    for r in 0..=max_radius {
        if best.is_some_and(|(d, _)| i64::from(r) * i64::from(r) >= d) {
            break;
        }
        for (dx, dy) in ring(r) {
            let (dx, dy) = if reverse { (-dx, -dy) } else { (dx, dy) };
            let tile = center.offset(dx, dy);
            if !accept(tile) {
                continue;
            }
            let d = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, tile));
            }
        }
    }
    best.map(|(_, tile)| tile)
}

/// The offsets of Chebyshev ring `r` in the spread-slot scan order: rows top
/// to bottom, columns left to right.
fn ring(r: i32) -> impl Iterator<Item = (i32, i32)> {
    let top = (-r..=r).map(move |dx| (dx, -r));
    let sides = (1 - r..r).flat_map(move |dy| [(-r, dy), (r, dy)]);
    let bottom = (-r..=r).map(move |dx| (dx, r)).filter(move |_| r > 0);
    top.chain(sides).chain(bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
    use crate::state::Faction;
    use crate::stats::UnitKind;
    use crate::{Event, Scenario};

    /// A sandbox over `map` with `seats` players and one unit per spec.
    fn world(map: &[&str], seats: usize, units: &[(u8, UnitKind, i32, i32)]) -> State {
        world_with_teams(map, &vec![None; seats], units)
    }

    /// A sandbox over `map` with one player per team entry.
    fn world_with_teams(
        map: &[&str],
        teams: &[Option<u8>],
        units: &[(u8, UnitKind, i32, i32)],
    ) -> State {
        let scenario = Scenario {
            mode: ScenarioMode::Sandbox,
            name: "reach".into(),
            seed: 3,
            map: map.iter().map(|row| (*row).to_owned()).collect(),
            players: teams
                .iter()
                .enumerate()
                .map(|(seat, &team)| PlayerSpec {
                    name: format!("p{seat}"),
                    faction: Faction::Ferrous,
                    team,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: units
                .iter()
                .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        };
        scenario.build().expect("reach fixture builds")
    }

    /// A 3x3 wall around an open pocket at (5, 3), centered on an 11x7 map.
    const POCKET: [&str; 7] = [
        "...........",
        "...........",
        "....###....",
        "....#.#....",
        "....###....",
        "...........",
        "...........",
    ];

    fn id(state: &State, slot: usize) -> UnitId {
        state.units[slot].id
    }

    #[test]
    fn rings_follow_the_spread_scan_order() {
        for r in 0i32..6 {
            let mut expected = Vec::new();
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) == r {
                        expected.push((dx, dy));
                    }
                }
            }
            assert_eq!(ring(r).collect::<Vec<_>>(), expected, "ring {r}");
        }
    }

    #[test]
    fn a_reachable_target_is_its_own_endpoint() {
        let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
        let mut reach = Reach::new(&state);
        let target = TilePos::new(9, 5);
        assert_eq!(
            reach.endpoint(&state, id(&state, 0), target, None),
            Some(target)
        );
        assert!(
            reach.endpoints.is_empty(),
            "no scan runs for a reachable target"
        );
    }

    #[test]
    fn a_cache_hit_equals_a_miss() {
        let state = world(
            &POCKET,
            1,
            &[(0, UnitKind::Sentinel, 1, 1), (0, UnitKind::Sentinel, 9, 1)],
        );
        let pocket = TilePos::new(5, 3);
        let mut shared = Reach::new(&state);
        let first = shared.endpoint(&state, id(&state, 0), pocket, None);
        let second = shared.endpoint(&state, id(&state, 1), pocket, None);
        assert_eq!(
            shared.endpoints.len(),
            1,
            "premise: both units share one component, target and frame"
        );
        assert_eq!(
            second,
            Reach::new(&state).endpoint(&state, id(&state, 1), pocket, None)
        );
        assert_eq!(
            first,
            Reach::new(&state).endpoint(&state, id(&state, 0), pocket, None)
        );
        assert_eq!(first, Some(TilePos::new(5, 1)));
    }

    #[test]
    fn mid_phase_depletion_reopens_the_ground() {
        let mut state = world(
            &[
                "...........",
                "....###....",
                "....#.s....",
                "....###....",
                "...........",
            ],
            1,
            &[(0, UnitKind::Sentinel, 1, 2)],
        );
        let walker = id(&state, 0);
        let pocket = TilePos::new(5, 2);
        let mut reach = Reach::new(&state);
        let sealed = reach.endpoint(&state, walker, pocket, None);
        assert!(sealed.is_some_and(|tile| tile != pocket));
        let node = TilePos::new(6, 2);
        while state.map.extract_scrap(node).is_some_and(|left| left > 0) {}
        assert!(state.passable(node), "premise: the node is gone");
        assert_eq!(
            reach.endpoint(&state, walker, pocket, None),
            sealed,
            "labels describe the ground they were built from"
        );
        reach.forget_ground();
        assert_eq!(reach.endpoint(&state, walker, pocket, None), Some(pocket));
    }

    #[test]
    fn mirrored_endpoints_are_symmetric() {
        let state = world(
            &POCKET,
            2,
            &[
                (0, UnitKind::Sentinel, 1, 3),
                (1, UnitKind::Sentinel, 9, 3),
                (0, UnitKind::Sentinel, 2, 5),
                (1, UnitKind::Sentinel, 8, 1),
            ],
        );
        let mirror = |tile: TilePos| TilePos::new(10 - tile.x, 6 - tile.y);
        let pocket = TilePos::new(5, 3);
        let mut reach = Reach::new(&state);
        for (left, right) in [(0, 1), (2, 3)] {
            let a = reach.endpoint(&state, id(&state, left), pocket, None);
            let b = reach.endpoint(&state, id(&state, right), mirror(pocket), None);
            assert_eq!(a.map(mirror), b, "units {left} and {right}");
            assert!(a.is_some_and(|tile| tile != pocket));
        }
        // Four shore tiles tie at distance two; each side keeps its own
        // frame's first rather than an absolute corner.
        assert_eq!(
            reach.endpoint(&state, id(&state, 0), pocket, None),
            Some(TilePos::new(5, 1))
        );
        assert_eq!(
            reach.endpoint(&state, id(&state, 1), pocket, None),
            Some(TilePos::new(5, 5))
        );
    }

    #[test]
    fn a_recompute_keeps_a_stored_endpoint_that_is_still_tied() {
        let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 3)]);
        let walker = id(&state, 0);
        let pocket = TilePos::new(5, 3);
        let mut reach = Reach::new(&state);
        let tied = TilePos::new(3, 3);
        assert_eq!(
            reach.endpoint(&state, walker, pocket, Some(tied)),
            Some(tied)
        );
        assert_eq!(
            reach.endpoint(&state, walker, pocket, Some(TilePos::new(5, 0))),
            Some(TilePos::new(5, 1)),
            "a farther stored tile yields to the nearest"
        );
        assert_eq!(
            reach.endpoint(&state, walker, pocket, Some(TilePos::new(4, 2))),
            Some(TilePos::new(5, 1)),
            "a closed stored tile yields to the nearest"
        );
    }

    #[test]
    fn a_blocked_start_whose_only_open_neighbour_is_diagonal_is_sealed() {
        let mut state = world(
            &[".##..", "###..", "###..", "....."],
            1,
            &[(0, UnitKind::Sentinel, 4, 3)],
        );
        let walker = id(&state, 0);
        state.units[0].pos = TilePos::new(1, 1).center();
        let mut reach = Reach::new(&state);
        for target in [TilePos::new(0, 0), TilePos::new(4, 3)] {
            assert_eq!(reach.endpoint(&state, walker, target, None), None);
            assert!(
                super::super::route_for(&state, UnitKind::Sentinel, TilePos::new(1, 1), target)
                    .is_none(),
                "A* agrees: no corner cut leaves the tile"
            );
        }
        // One open cardinal neighbour is enough.
        state.units[0].pos = TilePos::new(2, 1).center();
        assert_eq!(
            Reach::new(&state).endpoint(&state, walker, TilePos::new(4, 3), None),
            Some(TilePos::new(4, 3))
        );
    }

    #[test]
    fn an_off_map_target_clamps_onto_the_map() {
        let state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
        let walker = id(&state, 0);
        let mut reach = Reach::new(&state);
        for (target, clamped) in [
            (TilePos::new(-2_048, 3), TilePos::new(0, 3)),
            (TilePos::new(4, 2_048), TilePos::new(4, 6)),
            (TilePos::new(2_048, -2_048), TilePos::new(10, 0)),
        ] {
            assert_eq!(
                reach.endpoint(&state, walker, target, None),
                Some(clamped),
                "{target:?}"
            );
        }
    }

    #[test]
    fn an_off_map_unit_does_not_panic() {
        let mut state = world(&POCKET, 1, &[(0, UnitKind::Sentinel, 1, 1)]);
        let walker = id(&state, 0);
        state.units[0].pos = TilePos::new(-40, -40).center();
        let mut reach = Reach::new(&state);
        assert_eq!(
            reach.endpoint(&state, walker, TilePos::new(5, 3), None),
            None
        );
        state.units[0].pos = TilePos::new(-1, 2).center();
        assert_eq!(
            Reach::new(&state).endpoint(&state, walker, TilePos::new(5, 3), None),
            None,
            "an open neighbour across the edge is no way back"
        );
        assert!(
            super::super::route_for(
                &state,
                UnitKind::Sentinel,
                TilePos::new(-1, 2),
                TilePos::new(0, 2)
            )
            .is_none(),
            "A* agrees: no route starts off the map"
        );
    }

    #[test]
    fn a_crowd_chain_reaches_back_from_the_endpoint() {
        let mut state = world(
            &["..............", "..............", ".............."],
            1,
            &[
                (0, UnitKind::Sentinel, 1, 1),
                (0, UnitKind::Sentinel, 2, 1),
                (0, UnitKind::Sentinel, 3, 1),
                (0, UnitKind::Sentinel, 4, 1),
                (0, UnitKind::Sentinel, 9, 1),
                (0, UnitKind::Wisp, 5, 1),
            ],
        );
        // A line of touching parked bodies from the endpoint eastward.
        let step = UnitKind::Sentinel.stats().radius * 2;
        for slot in 0..4 {
            state.units[slot].pos = TilePos::new(1, 1).center()
                + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot as i32), Fx::ZERO);
        }
        let end = state.units[3].pos;
        state.units[4].pos = end + chassis::fx::Vec2Fx::new(step, Fx::ZERO);
        state.units[4].order = Order::Run {
            goal: TilePos::new(0, 1).into(),
        };
        state.units[5].pos = end + chassis::fx::Vec2Fx::new(step, Fx::ZERO);
        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        let endpoint = TilePos::new(1, 1);
        let mut reach = Reach::new(&state);
        assert!(reach.crowd_touches(&state, &index, id(&state, 4), endpoint));
        assert!(
            !reach.crowd_touches(&state, &index, id(&state, 5), endpoint),
            "a flier never touches a ground crowd"
        );
        // A body that starts walking mid-phase still counts: the crowd is the
        // one that stood when the phase began, whoever asks first.
        state.units[2].order = Order::Run {
            goal: TilePos::new(12, 1).into(),
        };
        assert!(reach.crowd_touches(&state, &index, id(&state, 4), endpoint));
        assert!(
            !Reach::new(&state).crowd_touches(&state, &index, id(&state, 4), endpoint),
            "the broken chain no longer reaches the endpoint"
        );
        state.units[4].pos = TilePos::new(12, 1).center();
        index.rebuild(&state.units);
        assert!(
            !Reach::new(&state).crowd_touches(&state, &index, id(&state, 4), endpoint),
            "a walker touching nothing has not arrived"
        );
        assert!(!reach.crowd_touches(&state, &index, UnitId(9_999), endpoint));
    }

    #[test]
    fn a_crowd_chain_counts_only_bodies_its_side_may_count() {
        // Seat 0 walks; seat 1 is its teammate and seat 2 is hostile. Bodies
        // 0-3 run east from the endpoint in touching steps, the first three
        // inside the arrival disk. Body 4 is the walker's own and touches
        // body 3; the walker touches only body 4.
        let run = |owners: [u8; 4]| {
            let mut units: Vec<(u8, UnitKind, i32, i32)> = owners
                .iter()
                .map(|&owner| (owner, UnitKind::Sentinel, 1, 1))
                .collect();
            units.push((0, UnitKind::Sentinel, 1, 1));
            units.push((0, UnitKind::Sentinel, 1, 1));
            let mut state = world_with_teams(
                &["..............", "..............", ".............."],
                &[Some(0), Some(0), Some(1)],
                &units,
            );
            let step = UnitKind::Sentinel.stats().radius * 2;
            for (slot, unit) in state.units.iter_mut().enumerate() {
                unit.pos = TilePos::new(1, 1).center()
                    + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot as i32), Fx::ZERO);
            }
            let walker = id(&state, 5);
            state.units[5].order = Order::Run {
                goal: TilePos::new(0, 1).into(),
            };
            let mut index = UnitIndex::new();
            index.rebuild(&state.units);
            Reach::new(&state).crowd_touches(&state, &index, walker, TilePos::new(1, 1))
        };
        assert!(run([0, 0, 0, 0]), "the walker's own crowd");
        assert!(run([1, 1, 1, 1]), "a teammate's crowd");
        assert!(
            run([2, 2, 0, 0]),
            "a friendly body inside the disk seeds it"
        );
        assert!(
            !run([2, 2, 2, 0]),
            "hostile bodies holding the whole disk seed nothing"
        );
        assert!(
            !run([0, 0, 2, 2]),
            "hostile bodies never link a friendly body to the disk"
        );
        assert!(
            !run([2, 2, 2, 2]),
            "nor does a hostile line reaching out from it"
        );
    }

    #[test]
    fn a_hostile_line_ends_no_walk_whether_or_not_its_side_sees_it() {
        // Parked bodies run east from the endpoint in touching steps to one
        // of the walker's own, which the walker touches. Seen from that end
        // of the line, the endpoint lies beyond sight.
        const LINE: usize = 17;
        let endpoint = TilePos::new(1, 1);
        let run = |line_owner: u8, spotter: bool| {
            let mut units = vec![(line_owner, UnitKind::Sentinel, 1, 1); LINE];
            units.extend([(0, UnitKind::Sentinel, 1, 1); 2]);
            if spotter {
                units.push((0, UnitKind::Wisp, 1, 1));
            }
            let mut state = world_with_teams(
                &["................................"; 3],
                &[Some(0), Some(1)],
                &units,
            );
            let step = UnitKind::Sentinel.stats().radius * 2;
            for (slot, unit) in state.units.iter_mut().take(LINE + 2).enumerate() {
                unit.pos = endpoint.center()
                    + chassis::fx::Vec2Fx::new(step * Fx::from_num(slot as i32), Fx::ZERO);
            }
            state.units[LINE + 1].order = Order::Run {
                goal: TilePos::new(0, 1).into(),
            };
            state.refresh_vision();
            let walker = id(&state, LINE + 1);
            let mut index = UnitIndex::new();
            index.rebuild(&state.units);
            let seen = state.vision(PlayerId(0)).visible(endpoint);
            let arrived = Reach::new(&state).crowd_touches(&state, &index, walker, endpoint);
            (seen, arrived)
        };
        assert_eq!(
            run(0, false),
            (true, true),
            "the side's own line ends the walk"
        );
        assert_eq!(
            run(1, false),
            (false, false),
            "a hostile line out of sight is not counted"
        );
        assert_eq!(
            run(1, true),
            (true, false),
            "nor is one in plain sight, so sight never changes the answer"
        );
    }

    #[test]
    fn a_depletion_during_the_brain_phase_reopens_the_ground_for_later_walkers() {
        // The scrap node at (6, 2) seals the pocket at (5, 2). A scout routes
        // first and builds the ground labels with the node closed; the
        // harvester then chips the node's last scrap before the walker plans.
        let mut state = world(
            &[
                "...........",
                "....###....",
                "....#.s....",
                "....###....",
                "...........",
            ],
            1,
            &[
                (0, UnitKind::Sentinel, 0, 0),
                (0, UnitKind::Harvester, 7, 2),
                (0, UnitKind::Sentinel, 10, 4),
            ],
        );
        let node = TilePos::new(6, 2);
        let pocket = TilePos::new(5, 2);
        while state.map.extract_scrap(node).is_some_and(|left| left > 1) {}
        let ticks_per_scrap = UnitKind::Harvester
            .stats()
            .harvest
            .expect("a harvester harvests")
            .ticks_per_scrap;
        let (scout, harvester, walker) = (id(&state, 0), id(&state, 1), id(&state, 2));
        state.units[0].order = Order::Run {
            goal: TilePos::new(0, 4).into(),
        };
        state.units[1].order = Order::Harvest {
            node,
            anchor: None,
            retiring: false,
        };
        state.units[1].pos = crate::geometry::work_approach_point(
            node.offset(1, 0),
            node,
            (1, 1),
            UnitKind::Harvester.stats().radius,
        );
        state.units[1].progress = ticks_per_scrap - 1;
        state.units[2].order = Order::Run {
            goal: pocket.into(),
        };
        assert_eq!(state.tick % 2, 0, "premise: the brains run in id order");
        assert!(scout < harvester && harvester < walker);
        let report = state.tick(&[]);
        assert!(
            report
                .events
                .iter()
                .any(|event| matches!(event, Event::NodeDepleted { pos } if *pos == node)),
            "premise: the harvester emptied the node this tick"
        );
        assert!(
            state.unit(scout).unwrap().path.is_some(),
            "premise: the scout routed before the node ran dry"
        );
        let unit = state.unit(walker).unwrap();
        assert_eq!(
            unit.order,
            Order::Run {
                goal: pocket.into()
            }
        );
        assert_eq!(unit.path.as_ref().map(|path| path.goal), Some(pocket));
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::OrderStalled { .. })),
            "nothing reports the pocket unreachable"
        );
    }
}
