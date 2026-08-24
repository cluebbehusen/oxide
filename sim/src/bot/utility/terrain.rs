//! Known-world routing, ferrying, and deterministic placement.

use super::*;

impl UtilityPolicy {
    /// Whether known ground connects `home` to any tile of the 2x2
    /// footprint anchored at `anchor`. BFS over tiles not known
    /// impassable (rock, mesa, pit — `known_rock` carries all three);
    /// unexplored tiles count open, the same optimism every founding
    /// walk uses. Runs only when a frame claim is otherwise ready, so
    /// the flood's cost is paid a handful of times per match.
    pub(super) fn ground_reaches(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| !obs.known_rock_at(t))
    }

    /// Whether a ground road from `home` to `anchor` is actually KNOWN:
    /// the same flood, but unexplored tiles count blocked. This is the
    /// ferry's and the mining arm's route question — a base only ever
    /// seen from the sky is an island war until a walked road proves
    /// otherwise, and the optimistic flood above can wander through any
    /// unexplored gulf forever without ever proving severance.
    pub(super) fn ground_route_known(obs: &Observation, home: TilePos, anchor: TilePos) -> bool {
        Self::ground_flood(obs, home, anchor, |t| {
            obs.explored(t) && !obs.known_rock_at(t)
        })
    }

    /// The shared reachability flood: BFS from `home` through tiles
    /// `enter` admits, looking for the 2x2 footprint at `anchor`.
    fn ground_flood(
        obs: &Observation,
        home: TilePos,
        anchor: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> bool {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 {
            return false;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let target = |t: TilePos| {
            (anchor.x..anchor.x + 2).contains(&t.x) && (anchor.y..anchor.y + 2).contains(&t.y)
        };
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        if !in_bounds(home) {
            return false;
        }
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            if target(t) {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        false
    }

    /// The nearest known enemy building no KNOWN ground road reaches —
    /// the island war's objective — or `None` while every known site
    /// has a walked road. Candidates are tried nearest-first by
    /// (manhattan, y, x). One flood of home's known-road component
    /// answers every candidate: per-site reachability from a fixed
    /// origin is component membership, and the per-site BFS this
    /// replaces re-walked the same component once per known enemy
    /// building on any connected map.
    pub(super) fn island_target(obs: &Observation, home: TilePos) -> Option<TilePos> {
        let mut sites: Vec<(i32, i32, i32)> = obs
            .enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .collect();
        sites.sort_unstable();
        if sites.is_empty() {
            return None;
        }
        let (w, h) = (obs.map_width, obs.map_height);
        let component =
            Self::ground_component(obs, home, |t| obs.explored(t) && !obs.known_rock_at(t));
        let footprint_reached = |anchor: TilePos| {
            component.as_ref().is_some_and(|seen| {
                (anchor.y..anchor.y + 2).any(|y| {
                    (anchor.x..anchor.x + 2).any(|x| {
                        (0..w).contains(&x) && (0..h).contains(&y) && seen[(y * w + x) as usize]
                    })
                })
            })
        };
        sites
            .into_iter()
            .map(|(_, y, x)| TilePos::new(x, y))
            .find(|anchor| !footprint_reached(*anchor))
    }

    /// Home's full walkable component under `enter`, as a seen-tile
    /// grid — the membership form of [`Self::ground_flood`], flooded to
    /// exhaustion. `None` when the map is degenerate or `home` is out
    /// of bounds, where the per-target flood reports nothing reachable.
    fn ground_component(
        obs: &Observation,
        home: TilePos,
        enter: impl Fn(TilePos) -> bool,
    ) -> Option<Vec<bool>> {
        let (w, h) = (obs.map_width, obs.map_height);
        if w <= 0 || h <= 0 || home.x < 0 || home.y < 0 || home.x >= w || home.y >= h {
            return None;
        }
        let idx = |t: TilePos| (t.y * w + t.x) as usize;
        let in_bounds = |t: TilePos| t.x >= 0 && t.y >= 0 && t.x < w && t.y < h;
        let mut seen = vec![false; (w * h) as usize];
        let mut open = std::collections::VecDeque::new();
        seen[idx(home)] = true;
        open.push_back(home);
        while let Some(t) = open.pop_front() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = t.offset(dx, dy);
                if in_bounds(n) && !seen[idx(n)] && enter(n) {
                    seen[idx(n)] = true;
                    open.push_back(n);
                }
            }
        }
        Some(seen)
    }

    /// A drop point beside the enemy base, from the target side's own
    /// known ground: the first ring-scanned tile ((r, y, x) order) that
    /// is not known rock, scrap, or a known building footprint —
    /// unexplored tiles count open, like every founding walk. The sim's
    /// unload scan handles exact placement around it; everything nearby
    /// known-blocked falls back to the anchor itself.
    fn unload_site(&self, obs: &Observation, target: TilePos) -> TilePos {
        for r in 2i32..=6 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = target.offset(dx, dy);
                    let in_bounds =
                        t.x >= 0 && t.y >= 0 && t.x < obs.map_width && t.y < obs.map_height;
                    if in_bounds && self.tile_open(obs, t) {
                        return t;
                    }
                }
            }
        }
        target
    }

    /// Ferry channel: when the known enemy base sits across ground no
    /// crawler can walk, run the Skyhook as
    /// a shuttle — gather a squad of idle ground fighters aboard, fly
    /// them to walkable ground beside the enemy base, and set them
    /// down. Landed machines are ordinary units again: the army channel
    /// drafts them where they stand and their own aggro carries the
    /// fight. The staging army's members are fair riders — on a gulf
    /// map the rally body IS the assault, and the Load lowering strikes
    /// riders from army bookkeeping — but the rear line and mid-march
    /// bodies are not.
    pub(super) fn ferry(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        enlisted: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        if !dials.ferry {
            return;
        }
        let Some(sky) = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().transport_capacity > 0)
            .min_by_key(|u| u.id)
        else {
            self.ferry_boarding.clear();
            return;
        };
        let Some(target) = Self::island_target(obs, home).or_else(|| {
            // Blind island desperation presumes the enemy at home's
            // mirror: the ferry flies at the one guess a symmetric
            // quarry offers, and contact does the rest.
            (self.desperate && !self.desperate_road)
                .then(|| TilePos::new(obs.map_width - 1 - home.x, obs.map_height - 1 - home.y))
        }) else {
            return;
        };
        // Riders gone from the field are aboard or dead; riders idle
        // again bounced off the sling. Either way, no longer pending.
        self.ferry_boarding
            .retain(|id| obs.my_units.iter().any(|u| u.id == *id && !u.idle));
        if sky.cargo > 0 {
            // Loaded, settled, and nobody still walking out: fly the
            // drop. A partial squad flies rather than waiting forever.
            if sky.idle && self.ferry_boarding.is_empty() {
                intents.push(Intent::Unload {
                    transport: sky.id,
                    at: self.unload_site(obs, target),
                });
            }
            return;
        }
        if !sky.idle {
            return; // outbound or returning
        }
        let staging: Vec<UnitId> = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .flat_map(|a| a.members.iter().copied())
            .collect();
        let pool: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == Domain::Ground
                    && stats.can_fight()
                    && stats.transport_size > 0
                    && u.idle
                    && (!enlisted.contains(&u.id) || staging.contains(&u.id))
            })
            .collect();
        if pool.len() < FERRY_SQUAD {
            return;
        }
        // Nearest to the sling first, ties to the lowest id; take what
        // fits the rack (a machine too big for the remaining room is
        // passed over for a smaller one behind it).
        let mut ranked: Vec<(i32, UnitId, u8)> = pool
            .iter()
            .map(|u| {
                (
                    u.tile.chebyshev(sky.tile),
                    u.id,
                    u.kind.stats().transport_size,
                )
            })
            .collect();
        ranked.sort_unstable();
        let mut room = sky.kind.stats().transport_capacity;
        let mut riders = Vec::new();
        for (_, id, size) in ranked {
            if size > 0 && size <= room {
                room -= size;
                riders.push(id);
            }
        }
        if riders.is_empty() {
            return;
        }
        self.ferry_boarding = riders.clone();
        intents.push(Intent::Load {
            transport: sky.id,
            riders,
        });
    }
    /// Nearest known scrap by (manhattan, y, x), skipping bounced nodes.
    pub(super) fn nearest_scrap(&self, obs: &Observation, from: TilePos) -> Option<TilePos> {
        obs.known_scrap
            .iter()
            .filter(|(pos, amount)| *amount > 0 && !self.dead_nodes.contains(pos))
            .map(|(pos, _)| (pos.manhattan(from), pos.y, pos.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
    }

    /// First anchor for `kind` ring-scanned outward from `near` whose
    /// footprint and doorstep ring are clear of everything the
    /// observation knows about — the sim's `can_place` still has the
    /// final word, and refusals land in [`Self::dead_anchors`].
    pub(super) fn placement_near(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        near: TilePos,
    ) -> Option<TilePos> {
        let (w, h) = kind.base_stats().size;
        for r in 3i32..=7 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let anchor = near.offset(dx, dy);
                    if self.placement_valid(obs, anchor, w, h) {
                        return Some(anchor);
                    }
                }
            }
        }
        None
    }

    fn placement_valid(&self, obs: &Observation, anchor: TilePos, width: i32, height: i32) -> bool {
        if self.dead_anchors.contains(&anchor) {
            return false;
        }
        let in_bounds = |tile: TilePos| {
            tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height
        };
        let footprint_ok = (0..width).all(|dx| {
            (0..height).all(|dy| {
                let tile = anchor.offset(dx, dy);
                in_bounds(tile) && obs.explored(tile) && self.placement_tile_open(obs, tile)
            })
        });
        if !footprint_ok {
            return false;
        }
        (-1..=width).any(|dx| {
            (-1..=height).any(|dy| {
                let core = (0..width).contains(&dx) && (0..height).contains(&dy);
                let tile = anchor.offset(dx, dy);
                !core && in_bounds(tile) && obs.explored(tile) && self.tile_open(obs, tile)
            })
        })
    }

    fn placement_tile_open(&self, obs: &Observation, tile: TilePos) -> bool {
        if !self.tile_open(obs, tile) {
            return false;
        }
        // Nothing may pave over a derelict Extractor frame: the sim
        // refuses the whole footprint as FrameBlocked, and an anchor the
        // scorer keeps proposing anyway feeds the dead-anchor ledger for
        // a refusal the bot could have predicted. (Frames are map data;
        // this check lives here rather than in `tile_open` because that
        // predicate also serves rally spots, where standing on a frame
        // is fine.)
        if obs.known_frames.iter().any(|frame| {
            tile.x >= frame.x && tile.x < frame.x + 2 && tile.y >= frame.y && tile.y < frame.y + 2
        }) {
            return false;
        }
        let claimed = obs.my_units.iter().any(|unit| {
            unit.founding.is_some_and(|(kind, anchor)| {
                let (width, height) = kind.base_stats().size;
                tile.x >= anchor.x
                    && tile.x < anchor.x + width
                    && tile.y >= anchor.y
                    && tile.y < anchor.y + height
            })
        });
        !claimed
            && !obs
                .enemy_units
                .iter()
                .any(|unit| unit.kind.stats().domain == Domain::Ground && unit.tile == tile)
    }

    /// Known-buildable: not rock, not scrap, not under any known
    /// building footprint.
    fn tile_open(&self, obs: &Observation, t: TilePos) -> bool {
        if self.rock_at(obs, t) || obs.known_scrap_at(t) {
            return false;
        }
        let covered = |b: &crate::bot::observation::BuildingObs| {
            let (w, h) = b.kind.base_stats().size;
            t.x >= b.anchor.x && t.x < b.anchor.x + w && t.y >= b.anchor.y && t.y < b.anchor.y + h
        };
        !obs.my_buildings.iter().any(covered)
            && !obs.ally_buildings.iter().any(covered)
            && !obs.enemy_buildings.iter().any(covered)
    }

    fn rock_at(&self, obs: &Observation, t: TilePos) -> bool {
        obs.known_rock_at(t)
    }

    /// The nearest known-open tile to `want` (spiral out to 3), for
    /// rally points that shouldn't sit inside a rock formation.
    pub(super) fn passable_near(&self, obs: &Observation, want: TilePos) -> TilePos {
        for r in 0i32..=3 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = want.offset(dx, dy);
                    if t.x >= 0
                        && t.y >= 0
                        && t.x < obs.map_width
                        && t.y < obs.map_height
                        && self.tile_open(obs, t)
                    {
                        return t;
                    }
                }
            }
        }
        want
    }
}
