//! Static defense: Turrets, Bastions and Flak Turrets beside the seat's most
//! valuable buildings, on the side threats come from, Arrays watching the
//! way in, Barricades ahead of the guns, Scuttle Charges on the approach,
//! upgrades for them, and a Repair Bay where the seat's wounded are. Each is an ordinary investment, worth what it adds to
//! the cover of those buildings' approaches. A short defense also buys one
//! defense at once where attackers find a building's approach uncovered.

use crate::composition;
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, footprint_centre, gap, ring};
use crate::investments::Investment;
use crate::map::MapModel;
use crate::memory::{Memory, SeenUnit};
use crate::placement;
use crate::profile::{PersonalityTraits, ResolvedProfile};
use crate::workers;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::stats::{
    BuildingStats, CHARGE_DAMAGE, Domain, FOUNDRY_REPAIR_PRICE, REPAIR_BAY_RADIUS,
};
use oxide_sim::{BuildingKind, UnitKind};
use std::cell::OnceCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// What one of several Foundries is worth guarding; a lone Foundry is worth
/// more, and nothing else is worth as much.
const FOUNDRY_VALUE: u64 = 12;

/// Tiles from a building's centre toward a threat sampled as its approach.
const APPROACH: [i64; 4] = [3, 6, 9, 12];

/// Tiles from a building's centre toward a threat an Array watches.
const FAR: [i64; 4] = [9, 12, 15, 18];

/// Tiles an Array's radar reaches.
const RADAR: i64 = 20;

/// Tiles from a Foundry's centre along its approach where its Scuttle
/// Charges start.
const MINEFIELD_START: i64 = 3;

/// Tiles either side of a Foundry's straight way in that its Scuttle Charges
/// cover: about a blast wide.
const MINEFIELD_WIDTH: i64 = 2;

/// Tiles apart Scuttle Charges stand, so one blast does not set off the
/// next.
const CHARGE_SPACING: i32 = 3;

/// Empty tiles between a gun and the Barricade in front of it.
const BARRICADE_GAP: i32 = 1;

/// Divides an obstacle's worth into investment points.
const OBSTACLE_POINTS: u64 = 16;

/// Empty tiles between a building and a defense guarding it.
const STANDOFF: [i32; 2] = [2, 3];

/// Scrap of missing health a Repair Bay's aura must reach to be worth
/// building.
const BAY_WOUNDS: u64 = 300;

/// Divides a Repair Bay's worth into investment points.
const BAY_POINTS: u64 = 2;

/// Divides a defense's worth into investment points.
const POINTS: u64 = 48;

/// Divides an upgrade's worth into investment points.
const UPGRADE_POINTS: u64 = 48;

/// Tiles beyond its reach inside which a threat keeps a defense from
/// upgrading: the building is down to a fifth of its health until done.
const UPGRADE_CLEARANCE: i32 = 3;

/// Ticks after which the opening economy counts as up even if harvesting is
/// not yet saturated.
pub(crate) const SETTLE_TICKS: u64 = 600;

/// Tiles around the nearest known enemy on an approach inside which its
/// companions count toward the threat along it.
const GROUP_TILES: i64 = 10;

/// What a seat's defenses must stand up to: at least an army at the stance's
/// minimum along any approach, and the per-mille margin an attack brings over
/// a defense it means to beat, so guns worth a threat divided by it hold that
/// threat off.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stakes {
    pub(crate) minimum: u64,
    pub(crate) margin: u64,
}

impl Stakes {
    pub(crate) fn of(profile: &ResolvedProfile) -> Self {
        Stakes {
            minimum: crate::missions::minimum(profile.stance),
            margin: crate::missions::margin(profile.difficulty),
        }
    }
}

#[cfg(test)]
impl Default for Stakes {
    /// A Standard, Balanced seat's.
    fn default() -> Self {
        Stakes {
            minimum: crate::missions::minimum(oxide_sim::scenario::BotStance::Balanced),
            margin: crate::missions::margin(oxide_sim::scenario::BotDifficulty::Standard),
        }
    }
}

/// Tiles from a building inside which visible attackers call for an
/// emergency defense.
const EMERGENCY_TILES: i64 = 12;

/// How sure the seat is of where a building's threat comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// Only public facts across a chasm: a hostile start or a known enemy
    /// building whose units could reach the building only by landing.
    Landing,
    /// Only public facts: a hostile start or a known enemy building.
    Prior,
    /// Enemies it remembers.
    Remembered,
    /// Enemies in sight now.
    Current,
}

impl Evidence {
    /// How much the evidence counts.
    fn weight(self) -> u64 {
        match self {
            Evidence::Landing | Evidence::Prior => 2,
            Evidence::Remembered => 4,
            Evidence::Current => 6,
        }
    }
}

/// Where a building is attacked from in one domain.
struct Approach {
    /// Points along the way in, in doubled coordinates.
    samples: Vec<(i64, i64)>,
    /// For each sample, in thousandths, how far own weapons covering it fall
    /// short of holding off the threat along the way: two thousand where none
    /// covers it, none once they hold.
    open: Vec<u64>,
    /// For each sample, whether own buildings see it.
    spotted: Vec<bool>,
    /// Points further along the way in, where an Array watches, and whether
    /// own buildings already see or pick up each.
    far: Vec<((i64, i64), bool)>,
    evidence: Evidence,
    /// The threat the samples lead to.
    source: (i64, i64),
}

/// A building worth guarding.
struct Asset {
    anchor: TilePos,
    size: (i32, i32),
    centre: (i64, i64),
    value: u64,
    ground: Option<Approach>,
    air: Option<Approach>,
}

impl Asset {
    fn approach(&self, domain: Domain) -> Option<&Approach> {
        match domain {
            Domain::Ground => self.ground.as_ref(),
            Domain::Air => self.air.as_ref(),
        }
    }
}

/// Where a weapon reaches, in doubled coordinates.
#[derive(Clone, Copy)]
struct Cover {
    centre: (i64, i64),
    /// What the weapon cost.
    value: u64,
    /// Squared doubled reach, and squared doubled minimum range.
    reach2: i64,
    min2: i64,
    ground: bool,
    air: bool,
}

impl Cover {
    fn of(stats: &BuildingStats, kind: BuildingKind, anchor: TilePos) -> Option<Self> {
        let mut cover = Cover {
            centre: footprint_centre(kind, anchor),
            value: kind
                .base_stats()
                .construction
                .as_ref()
                .map_or(0, |construction| u64::from(construction.cost)),
            reach2: 0,
            min2: i64::MAX,
            ground: false,
            air: false,
        };
        for weapon in stats.weapons {
            let reach = (weapon.range + weapon.range).to_num::<i64>();
            let min = (weapon.minimum_range + weapon.minimum_range).to_num::<i64>();
            cover.reach2 = cover.reach2.max(reach * reach);
            cover.min2 = cover.min2.min(min * min);
            cover.ground |= weapon.targets.ground;
            cover.air |= weapon.targets.air;
        }
        (cover.ground || cover.air).then_some(cover)
    }

    fn covers(self, domain: Domain, point: (i64, i64)) -> bool {
        let fires = match domain {
            Domain::Ground => self.ground,
            Domain::Air => self.air,
        };
        let distance2 = (self.centre.0 - point.0).pow(2) + (self.centre.1 - point.1).pow(2);
        fires && distance2 <= self.reach2 && distance2 >= self.min2
    }
}

/// What one decision knows about guarding the seat's buildings.
struct Guard<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    memory: &'a Memory,
    frame: HomeFrame,
    assets: Vec<Asset>,
    /// The seat's own armed buildings, built or not.
    covers: Vec<Cover>,
    /// Ground the seat's Harvesters stand on: only there can it build.
    crews: Vec<u32>,
    /// Whether the opening economy is up. Before then, a defense against a
    /// threat known only from public facts would take scrap and a Harvester
    /// the economy still needs.
    settled: bool,
    /// Whether the seat's army is too small to hold on its own, so a threat
    /// known only from public facts weighs as much as a remembered one.
    exposed: bool,
    stakes: Stakes,
}

impl<'a> Guard<'a> {
    fn new(
        observation: &'a ObservationData,
        map: &'a MapModel,
        memory: &'a Memory,
        settled: bool,
        exposed: bool,
        stakes: Stakes,
    ) -> Option<Self> {
        let frame = HomeFrame::of(observation, map)?;
        let covers: Vec<Cover> = observation
            .my_buildings
            .iter()
            .filter(|building| !building.provisional)
            .filter_map(|building| {
                Cover::of(
                    building.kind.tier_stats(building.tier),
                    building.kind,
                    building.anchor,
                )
            })
            .collect();
        let sight: Vec<((i64, i64), i64)> = observation
            .my_buildings
            .iter()
            .chain(&observation.ally_buildings)
            .filter(|building| building.built)
            .map(|building| {
                let vision = i64::from(building.kind.tier_stats(building.tier).vision);
                let reach = if building.kind == BuildingKind::Array {
                    vision.max(RADAR)
                } else {
                    vision
                };
                (
                    footprint_centre(building.kind, building.anchor),
                    4 * reach * reach,
                )
            })
            .collect();
        let seen = |point: (i64, i64)| {
            sight
                .iter()
                .any(|(centre, reach2)| distance2(*centre, point) <= *reach2)
        };
        let current = in_sight(observation, map);
        let mut assets = assets(observation, frame);
        let mut walkers: Vec<Walkers> = Vec::new();
        for asset in &assets {
            let around = grounds(map, asset.anchor, asset.size);
            if !walkers.iter().any(|walkers| walkers.grounds == around) {
                walkers.push(Walkers {
                    grounds: around,
                    at: OnceCell::new(),
                });
            }
        }
        let known = Known {
            observation,
            map,
            memory,
            frame,
            exposed,
            current: &current,
            bases: OnceCell::new(),
            aircraft: OnceCell::new(),
            walkers,
        };
        // Many buildings share a threat's source.
        let mut threats: Vec<((i64, i64), Domain, u64)> = Vec::new();
        for asset in &mut assets {
            let centre = asset.centre;
            asset.ground = approach(&known, asset, Domain::Ground);
            asset.air = approach(&known, asset, Domain::Air);
            for (domain, approach) in [
                (Domain::Ground, asset.ground.as_mut()),
                (Domain::Air, asset.air.as_mut()),
            ] {
                let Some(approach) = approach else {
                    continue;
                };
                let known = threats
                    .iter()
                    .find(|(source, of, _)| *source == approach.source && *of == domain)
                    .map(|(_, _, value)| *value);
                let value = known.unwrap_or_else(|| {
                    let value = threat(memory, observation.tick, approach.source, domain, stakes);
                    threats.push((approach.source, domain, value));
                    value
                });
                let need = value * 1_000 / stakes.margin.max(1);
                approach.open = approach
                    .samples
                    .iter()
                    .map(|point| {
                        let covered: u64 = covers
                            .iter()
                            .filter(|cover: &&Cover| cover.covers(domain, *point))
                            .map(|cover| cover.value)
                            .sum();
                        (2_000 * need.saturating_sub(covered))
                            .checked_div(need)
                            .unwrap_or(0)
                    })
                    .collect();
                approach.spotted = approach.samples.iter().map(|point| seen(*point)).collect();
                approach.far = along(centre, approach.source, &FAR)
                    .into_iter()
                    .map(|point| (point, seen(point)))
                    .collect();
            }
        }
        let mut crews: Vec<u32> = observation
            .my_units
            .iter()
            .filter(|unit| workers::worker(unit.kind))
            .filter_map(|unit| map.component(unit.tile))
            .collect();
        crews.sort_unstable();
        crews.dedup();
        Some(Guard {
            observation,
            map,
            memory,
            frame,
            assets,
            covers,
            crews,
            settled,
            exposed,
            stakes,
        })
    }

    /// How much a threat's evidence counts for a defense of `kind`. A seat
    /// without an army to speak of guards against the hostile start almost as
    /// if it had seen enemies, and against a landing with a Turret.
    fn weight(&self, approach: &Approach, kind: BuildingKind) -> u64 {
        match approach.evidence {
            Evidence::Prior if self.exposed => 5,
            Evidence::Landing if kind == BuildingKind::Turret => 5,
            evidence => evidence.weight(),
        }
    }

    /// Own weapons covering `point` in `domain`.
    fn covered(&self, domain: Domain, point: (i64, i64)) -> usize {
        self.covers
            .iter()
            .filter(|cover| cover.covers(domain, point))
            .count()
    }

    /// What a defense of `kind` at `anchor` adds to `asset`'s approach, in
    /// thousandths: how far the samples it covers still fall short of the
    /// threat along the way. A Bastion counts only samples its owner's or an
    /// ally's buildings see, since it fires no further than something spots
    /// for it. An Array adds two thousand for each point further along the way
    /// in that nothing sees yet.
    fn gain(
        &self,
        asset: &Asset,
        kind: BuildingKind,
        anchor: TilePos,
        reach: Option<Cover>,
    ) -> u64 {
        if kind == BuildingKind::Array {
            let centre = footprint_centre(kind, anchor);
            return 2_000
                * self.watched(asset, |point, seen| {
                    !seen && distance2(centre, point) <= 4 * RADAR * RADAR
                });
        }
        let Some(reach) = reach else {
            return 0;
        };
        let cover = Cover {
            centre: footprint_centre(kind, anchor),
            ..reach
        };
        let domain = domain(kind);
        let Some(approach) = asset.approach(domain) else {
            return 0;
        };
        let bastion = kind == BuildingKind::Bastion;
        approach
            .samples
            .iter()
            .zip(&approach.open)
            .zip(&approach.spotted)
            .filter(|((point, _), spotted)| {
                cover.covers(domain, **point) && (!bastion || **spotted)
            })
            .map(|((_, open), _)| *open)
            .sum()
    }

    /// Points far along `asset`'s ways in, from the ground and the air, that
    /// `counts`, given each point and whether own buildings see it.
    fn watched(&self, asset: &Asset, counts: impl Fn((i64, i64), bool) -> bool) -> u64 {
        [Domain::Ground, Domain::Air]
            .into_iter()
            .filter_map(|domain| asset.approach(domain))
            .flat_map(|approach| &approach.far)
            .filter(|(point, seen)| counts(*point, *seen))
            .count() as u64
    }

    /// Whether prior evidence may count yet.
    fn usable(&self, approach: &Approach) -> bool {
        !matches!(approach.evidence, Evidence::Landing | Evidence::Prior) || self.settled
    }

    /// The best spot for a Barricade: a tile's gap in front of an own Turret
    /// or Bastion without one, toward the threat of the building it guards.
    /// Whether it leaves the seat's paths open is checked when it is bought.
    fn barricade(&self) -> Option<(TilePos, u64)> {
        let observation = self.observation;
        let barricades: Vec<&BuildingObs> = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Barricade)
            .collect();
        let mut sites = Vec::new();
        for gun in observation.my_buildings.iter().filter(|building| {
            building.built && matches!(building.kind, BuildingKind::Turret | BuildingKind::Bastion)
        }) {
            let size = gun.kind.base_stats().size;
            let fronted = barricades.iter().any(|barricade| {
                gap(gun.anchor, size, barricade.anchor, (1, 1)) <= BARRICADE_GAP + 1
            });
            if fronted {
                continue;
            }
            let centre = footprint_centre(gun.kind, gun.anchor);
            let Some((asset, approach)) = self
                .assets
                .iter()
                .filter_map(|asset| Some((asset, asset.approach(Domain::Ground)?)))
                .filter(|(_, approach)| self.usable(approach))
                .min_by_key(|(asset, _)| (distance2(asset.centre, centre), asset.centre))
            else {
                continue;
            };
            let worth = asset.value * 2 * self.weight(approach, BuildingKind::Barricade);
            let low = -(BARRICADE_GAP + 1);
            let high = size.0.max(size.1) + BARRICADE_GAP;
            for tile in (low..=high)
                .flat_map(|dy| (low..=high).map(move |dx| gun.anchor.offset(dx, dy)))
                .filter(|tile| gap(gun.anchor, size, *tile, (1, 1)) == BARRICADE_GAP)
            {
                let point = doubled(tile);
                let ahead = (point.0 - centre.0) * (approach.source.0 - centre.0)
                    + (point.1 - centre.1) * (approach.source.1 - centre.1);
                if ahead > 0 {
                    sites.push((worth, self.frame.rank(approach.source, point), tile));
                }
            }
        }
        first_placeable(sites, |tile| self.placeable(BuildingKind::Barricade, tile))
    }

    /// The best spot for a Scuttle Charge: on the straight way in to a
    /// Foundry whose field still falls short of its threat, nearest the
    /// Foundry, clear of other own charges. A field holds enough charges to
    /// deal the health of the threat along the way divided by the margin, one
    /// body to a blast; each spot is worth the share of that still missing.
    fn charge(&self) -> Option<(TilePos, u64)> {
        let observation = self.observation;
        let charges: Vec<TilePos> = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::ScuttleCharge)
            .map(|building| building.anchor)
            .collect();
        let mut sites = Vec::new();
        for asset in self
            .assets
            .iter()
            .filter(|asset| asset.value >= FOUNDRY_VALUE)
        {
            let Some(approach) = asset.approach(Domain::Ground) else {
                continue;
            };
            if !self.usable(approach) {
                continue;
            }
            let health = health(self.memory, observation.tick, approach.source, self.stakes);
            let need =
                (health * 1_000 / self.stakes.margin.max(1)).div_ceil(u64::from(CHARGE_DAMAGE));
            let field = Field::of(asset.centre, approach.source);
            let laid = charges
                .iter()
                .filter(|charge| field.holds(doubled(**charge)))
                .count() as u64;
            if laid >= need {
                continue;
            }
            let worth = asset.value
                * 2
                * self.weight(approach, BuildingKind::ScuttleCharge)
                * (need - laid)
                / need;
            let Some((row, tile)) = field.first(
                |tile| {
                    charges
                        .iter()
                        .all(|charge| charge.chebyshev(tile) >= CHARGE_SPACING)
                        && self.placeable(BuildingKind::ScuttleCharge, tile)
                },
                |tile| self.frame.rank(approach.source, doubled(tile)),
            ) else {
                continue;
            };
            let rank = self.frame.rank(approach.source, doubled(tile));
            sites.push((Reverse(worth), row, rank, tile));
        }
        sites
            .into_iter()
            .min()
            .map(|(Reverse(worth), _, _, tile)| (tile, worth))
    }

    /// The best spot for a Repair Bay beside a guarded building: where its
    /// aura reaches the most missing value among the
    /// seat's wounded ground units and damaged buildings that no Repair Bay
    /// reaches yet, as its anchor and that value.
    fn bay(&self) -> Option<(TilePos, u64)> {
        let observation = self.observation;
        let size = BuildingKind::RepairBay.base_stats().size;
        let bays: Vec<TilePos> = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::RepairBay)
            .map(|building| building.anchor)
            .collect();
        let units = observation
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().domain == Domain::Ground)
            .map(|unit| {
                let stats = unit.kind.stats();
                let (x, y) = doubled(unit.tile);
                (
                    Span {
                        x: (x, x),
                        y: (y, y),
                    },
                    stats.cost,
                    unit.hp,
                    stats.max_hp,
                )
            });
        let buildings = observation
            .my_buildings
            .iter()
            .filter(|building| building.built && building.kind != BuildingKind::RepairBay)
            .map(|building| {
                let stats = building.kind.tier_stats(building.tier);
                let price = building
                    .kind
                    .base_stats()
                    .construction
                    .as_ref()
                    .map_or(FOUNDRY_REPAIR_PRICE, |construction| construction.cost);
                let span = Span::of(building.kind.base_stats().size, building.anchor);
                (span, price, building.hp, stats.max_hp)
            });
        let wounds: Vec<(Span, u64)> = units
            .chain(buildings)
            .filter(|(_, _, hp, max)| hp < max)
            .map(|(span, price, hp, max)| {
                let max = u64::from(max.max(1));
                let missing = u64::from(price) * (max - u64::from(hp).min(max)) / max;
                (span, missing)
            })
            .filter(|(span, _)| !bays.iter().any(|bay| Span::of(size, *bay).aura(*span)))
            .collect();
        if wounds.iter().map(|(_, missing)| missing).sum::<u64>() < BAY_WOUNDS {
            return None;
        }
        let mut sites = Vec::new();
        let margin = 2 * i64::from(STANDOFF[1] + size.0.max(size.1));
        for asset in &self.assets {
            // Every site around the asset lies within this span.
            let span = Span::of(asset.size, asset.anchor);
            let around = Span {
                x: (span.x.0 - margin, span.x.1 + margin),
                y: (span.y.0 - margin, span.y.1 + margin),
            };
            let near: Vec<&(Span, u64)> = wounds
                .iter()
                .filter(|(span, _)| around.aura(*span))
                .collect();
            if near.iter().map(|(_, missing)| missing).sum::<u64>() < BAY_WOUNDS {
                continue;
            }
            for anchor in sites_around(asset, size) {
                let bay = Span::of(size, anchor);
                let reached: u64 = near
                    .iter()
                    .filter(|(span, _)| bay.aura(*span))
                    .map(|(_, missing)| missing)
                    .sum();
                if reached >= BAY_WOUNDS {
                    let centre = footprint_centre(BuildingKind::RepairBay, anchor);
                    sites.push((reached, self.frame.rank(asset.centre, centre), anchor));
                }
            }
        }
        first_placeable(sites, |anchor| {
            self.placeable(BuildingKind::RepairBay, anchor)
        })
    }

    /// Whether `kind` may go at `anchor` as far as the seat knows, on ground
    /// a Harvester of the seat stands on, and was not refused there lately.
    fn placeable(&self, kind: BuildingKind, anchor: TilePos) -> bool {
        let crewed = self
            .map
            .component(anchor)
            .is_some_and(|ground| self.crews.contains(&ground));
        crewed
            && !self.memory.failed(kind, anchor, self.observation.tick)
            && placement::check(self.observation, kind, anchor, &[]).is_ok()
    }

    /// The best site for `kind` guarding any asset, as its anchor and worth:
    /// asset value times gain times evidence, placeable as far as the seat
    /// knows and not recently refused. Equal sites go to the one nearest a
    /// threat the seat has seen, or nearest the building when the threat is
    /// only a guess from public facts. Sites are tried best first, so few
    /// placement checks run.
    fn best(&self, kind: BuildingKind) -> Option<(TilePos, u64)> {
        self.best_where(kind, |_| true)
    }

    /// The best site for `kind` guarding one of the assets `guarded` allows.
    fn best_where(
        &self,
        kind: BuildingKind,
        guarded: impl Fn(&Asset) -> bool,
    ) -> Option<(TilePos, u64)> {
        let size = kind.base_stats().size;
        let reach = Cover::of(kind.base_stats(), kind, TilePos::new(0, 0));
        let bastion = kind == BuildingKind::Bastion;
        // Each asset with the most any site beside it could be worth: every
        // sample still open, or every far point nobody sees.
        let mut candidates: Vec<(u64, &Asset, &Approach)> = Vec::new();
        for asset in self.assets.iter().filter(|asset| guarded(asset)) {
            // Radar watches for aircraft as well as ground units, so an
            // Array faces the air approach where no ground one is known.
            let approach = asset
                .approach(domain(kind))
                .or_else(|| (kind == BuildingKind::Array).then(|| asset.approach(Domain::Air))?);
            let Some(approach) = approach else {
                continue;
            };
            let most = if kind == BuildingKind::Array {
                2_000 * self.watched(asset, |_, seen| !seen)
            } else {
                approach
                    .open
                    .iter()
                    .zip(&approach.spotted)
                    .filter(|(_, spotted)| !bastion || **spotted)
                    .map(|(open, _)| *open)
                    .sum()
            };
            if !self.usable(approach) || most == 0 {
                continue;
            }
            let bound = asset.value * most * self.weight(approach, kind) / 1_000;
            candidates.push((bound, asset, approach));
        }
        candidates.sort_by_key(|(bound, _, _)| Reverse(*bound));
        // Sites best worth first, then by rank. Once the best placeable one
        // is worth more than any asset still to come could offer, it wins.
        let mut sites: BinaryHeap<Reverse<(Reverse<u64>, _, TilePos)>> = BinaryHeap::new();
        for (bound, asset, approach) in candidates {
            while let Some(Reverse((Reverse(worth), _, anchor))) = sites.peek() {
                if !self.placeable(kind, *anchor) {
                    sites.pop();
                } else if *worth > bound {
                    return Some((*anchor, *worth));
                } else {
                    break;
                }
            }
            for anchor in sites_around(asset, size) {
                let centre = footprint_centre(kind, anchor);
                let facing = (centre.0 - asset.centre.0) * (approach.source.0 - asset.centre.0)
                    + (centre.1 - asset.centre.1) * (approach.source.1 - asset.centre.1)
                    > 0;
                if !facing {
                    continue;
                }
                let worth = asset.value
                    * self.gain(asset, kind, anchor, reach)
                    * self.weight(approach, kind)
                    / 1_000;
                if worth > 0 {
                    let toward = match approach.evidence {
                        Evidence::Landing | Evidence::Prior => asset.centre,
                        Evidence::Remembered | Evidence::Current => approach.source,
                    };
                    let rank = self.frame.rank(toward, centre);
                    sites.push(Reverse((Reverse(worth), rank, anchor)));
                }
            }
        }
        while let Some(Reverse((Reverse(worth), _, anchor))) = sites.pop() {
            if self.placeable(kind, anchor) {
                return Some((anchor, worth));
            }
        }
        None
    }
}

/// The first of `sites`, best worth first and then by rank, that
/// `placeable` allows, with its worth. A heap yields them in that order
/// without sorting the many that are never tried.
fn first_placeable<R: Ord>(
    sites: Vec<(u64, R, TilePos)>,
    placeable: impl Fn(TilePos) -> bool,
) -> Option<(TilePos, u64)> {
    let mut heap: BinaryHeap<Reverse<(Reverse<u64>, R, TilePos)>> = sites
        .into_iter()
        .map(|(worth, rank, anchor)| Reverse((Reverse(worth), rank, anchor)))
        .collect();
    while let Some(Reverse((Reverse(worth), _, anchor))) = heap.pop() {
        if placeable(anchor) {
            return Some((anchor, worth));
        }
    }
    None
}

/// Voluntary defenses, defense upgrades and Repair Bays worth investing in,
/// with their scores. Personality weighs each kind: fortification for
/// Turrets, Bastions and Barricades, fortification and support for Flak
/// Turrets, fortification and guile for Arrays and Scuttle Charges,
/// fortification and greed for upgrades, support for Repair Bays.
pub(crate) fn investments(
    observation: &ObservationData,
    map: &MapModel,
    memory: &Memory,
    traits: PersonalityTraits,
    settled: bool,
    exposed: bool,
    stakes: Stakes,
) -> Vec<(Investment, u32)> {
    let Some(guard) = Guard::new(observation, map, memory, settled, exposed, stakes) else {
        return Vec::new();
    };
    let fortification = u64::from(traits.fortification);
    let cunning = (fortification + u64::from(traits.guile)) / 2;
    let weights = [
        (BuildingKind::Turret, fortification),
        (BuildingKind::Bastion, fortification * 6 / 5),
        (
            BuildingKind::FlakTurret,
            (fortification + u64::from(traits.support)) / 2,
        ),
        (BuildingKind::Array, cunning / 2),
    ];
    let mut list: Vec<(Investment, u32)> = weights
        .into_iter()
        .filter_map(|(kind, weight)| {
            let (anchor, worth) = guard.best(kind)?;
            Some((
                Investment::Defense { kind, anchor },
                points(worth * weight / POINTS),
            ))
        })
        .collect();
    if let Some((anchor, worth)) = guard.barricade() {
        let kind = BuildingKind::Barricade;
        list.push((
            Investment::Defense { kind, anchor },
            points(worth * fortification / OBSTACLE_POINTS),
        ));
    }
    let fabricator = observation
        .my_buildings
        .iter()
        .any(|building| building.kind == BuildingKind::Fabricator && building.built);
    if fabricator && let Some((anchor, worth)) = guard.charge() {
        let kind = BuildingKind::ScuttleCharge;
        list.push((
            Investment::Defense { kind, anchor },
            points(worth * cunning / OBSTACLE_POINTS),
        ));
    }
    if let Some((anchor, wounds)) = guard.bay() {
        let kind = BuildingKind::RepairBay;
        list.push((
            Investment::Defense { kind, anchor },
            points(wounds * composition::weight(traits.support) / 1_000 / BAY_POINTS),
        ));
    }
    let upgrade_weight = (fortification + u64::from(traits.greed)) / 2;
    list.extend(
        observation
            .my_buildings
            .iter()
            .filter_map(|building| upgrade(&guard, building, upgrade_weight)),
    );
    list
}

/// An upgrade for a built defense, worth the approach samples it covers, or
/// for an Array the far points its radar watches, each by how sure the seat
/// is of the threat, unless an enemy in sight could hit it while it is down,
/// from the defense's reach or its own, or the next tier needs a building the
/// seat has not built.
fn upgrade(guard: &Guard<'_>, building: &BuildingObs, weight: u64) -> Option<(Investment, u32)> {
    let observation = guard.observation;
    if !building.built {
        return None;
    }
    let stats = building.kind.tier_stats(building.tier);
    let cover = Cover::of(stats, building.kind, building.anchor);
    let array = building.kind == BuildingKind::Array;
    if cover.is_none() && !array {
        return None;
    }
    let next = building.kind.upgrade_from(building.tier)?;
    let ready = next.requires.iter().all(|required| {
        observation
            .my_buildings
            .iter()
            .any(|own| own.kind == *required && own.built)
    });
    let reach = stats
        .weapons
        .iter()
        .map(|weapon| weapon.range.ceil().to_num::<i32>())
        .max()
        .unwrap_or(0);
    let threatened = observation.enemy_units.iter().any(|enemy| {
        let Some(strike) = enemy
            .kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground)
            .map(|weapon| weapon.range.ceil().to_num::<i32>())
            .max()
        else {
            return false;
        };
        gap(
            building.anchor,
            building.kind.base_stats().size,
            enemy.tile,
            (1, 1),
        ) < reach.max(strike) + UPGRADE_CLEARANCE
    });
    if !ready || threatened {
        return None;
    }
    let centre = footprint_centre(building.kind, building.anchor);
    let worth: u64 = guard
        .assets
        .iter()
        .map(|asset| match cover {
            Some(cover) => [Domain::Ground, Domain::Air]
                .into_iter()
                .filter_map(|domain| Some((domain, asset.approach(domain)?)))
                .map(|(domain, approach)| {
                    // How far the samples it covers still fall short of
                    // holding: toward two each where it stands alone, none
                    // once they hold.
                    let short: u64 = approach
                        .samples
                        .iter()
                        .zip(&approach.open)
                        .filter(|(point, _)| cover.covers(domain, **point))
                        .map(|(_, open)| *open)
                        .sum();
                    asset.value * short * approach.evidence.weight() / 1_000
                })
                .sum::<u64>(),
            None => {
                let evidence = [Domain::Ground, Domain::Air]
                    .into_iter()
                    .filter_map(|domain| asset.approach(domain))
                    .map(|approach| approach.evidence.weight())
                    .max()
                    .unwrap_or(0);
                let watched = guard.watched(asset, |point, _| {
                    distance2(centre, point) <= 4 * RADAR * RADAR
                });
                asset.value * watched * evidence
            }
        })
        .sum();
    (worth > 0).then(|| {
        (
            Investment::Upgrade {
                building: building.id,
                tier: building.tier + 1,
            },
            points(worth * weight / UPGRADE_POINTS),
        )
    })
}

/// Buys a defense now beside each valuable building whose approach visible
/// attackers near it find uncovered: a Turret against ground attackers, a
/// Flak Turret against aircraft. Each such building has one standing
/// unfinished at a time.
pub(crate) fn emergency(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    memory: &Memory,
    stakes: Stakes,
    ledger: &mut Ledger,
) {
    let Some(guard) = Guard::new(observation, map, memory, true, true, stakes) else {
        return;
    };
    for (domain, kind) in [
        (Domain::Ground, BuildingKind::Turret),
        (Domain::Air, BuildingKind::FlakTurret),
    ] {
        let size = kind.base_stats().size;
        // An unfinished one beside the building already answers it.
        let unanswered = |asset: &Asset| {
            !observation.my_buildings.iter().any(|building| {
                building.kind == kind
                    && !building.built
                    && gap(asset.anchor, asset.size, building.anchor, size) <= STANDOFF[1]
            })
        };
        let pressed = |asset: &Asset| {
            asset.approach(domain).is_some_and(|approach| {
                approach.evidence == Evidence::Current
                    && chebyshev(approach.source, asset.centre) <= 2 * EMERGENCY_TILES
                    && approach
                        .samples
                        .iter()
                        .all(|point| guard.covered(domain, *point) == 0)
            })
        };
        let price = kind
            .base_stats()
            .construction
            .as_ref()
            .map_or(u32::MAX, |construction| construction.cost);
        for asset in guard
            .assets
            .iter()
            .filter(|asset| pressed(asset) && unanswered(asset))
        {
            let Some((anchor, _)) = guard.best_where(kind, |other| other.anchor == asset.anchor)
            else {
                continue;
            };
            let Ok(allowed) = placement::check(observation, kind, anchor, ledger.planned()) else {
                continue;
            };
            let centre = footprint_centre(kind, anchor);
            if ledger.spendable() < price {
                return;
            }
            if let Some(builder) = workers::builder(observation, map, frame, anchor, centre, ledger)
            {
                ledger.build(builder, kind, anchor, allowed.defer, price);
            }
        }
    }
}

/// The value of the armed enemies in `domain` the seat remembers near
/// `source`, by how sure it is they are still there, and at least an army at
/// the stance's minimum.
fn threat(memory: &Memory, now: u64, source: (i64, i64), domain: Domain, stakes: Stakes) -> u64 {
    let near: u64 = armed_near(memory, source, domain)
        .map(|unit| unit.value(now))
        .sum();
    near.max(stakes.minimum)
}

/// The health of the armed ground enemies the seat remembers near `source`,
/// by how sure it is they are still there, and at least an army of Sentinels
/// at the stance's minimum.
fn health(memory: &Memory, now: u64, source: (i64, i64), stakes: Stakes) -> u64 {
    let near: u64 = armed_near(memory, source, Domain::Ground)
        .map(|unit| u64::from(unit.kind.stats().max_hp) * u64::from(unit.confidence(now)) / 1_000)
        .sum();
    let sentinel = UnitKind::Sentinel.stats();
    near.max(stakes.minimum * u64::from(sentinel.max_hp) / u64::from(sentinel.cost))
}

/// The armed enemies in `domain` the seat remembers near `source`.
fn armed_near(
    memory: &Memory,
    source: (i64, i64),
    domain: Domain,
) -> impl Iterator<Item = &SeenUnit> {
    memory.units().iter().filter(move |unit| {
        let stats = unit.kind.stats();
        !stats.weapons.is_empty()
            && stats.domain == domain
            && chebyshev(doubled(unit.tile), source) <= 2 * GROUP_TILES
    })
}

/// An enemy unit in sight that fights or carries others.
struct Enemy {
    at: (i64, i64),
    domain: Domain,
    /// Ground beside a ground unit. Few units are in sight, and every guarded
    /// building asks about each of them.
    grounds: Vec<u32>,
}

/// The enemy units in sight that fight or carry others.
fn in_sight(observation: &ObservationData, map: &MapModel) -> Vec<Enemy> {
    observation
        .enemy_units
        .iter()
        .filter(|unit| attacker(unit.kind))
        .map(|unit| {
            let domain = unit.kind.stats().domain;
            let grounds = match domain {
                Domain::Ground => grounds(map, unit.tile, (1, 1)),
                Domain::Air => Vec::new(),
            };
            Enemy {
                at: doubled(unit.tile),
                domain,
                grounds,
            }
        })
        .collect()
}

/// Whether `kind` fights or carries others.
fn attacker(kind: UnitKind) -> bool {
    let stats = kind.stats();
    !stats.weapons.is_empty() || stats.transport_capacity > 0
}

/// The ground around a `size` footprint at `anchor`.
fn grounds(map: &MapModel, anchor: TilePos, size: (i32, i32)) -> Vec<u32> {
    let mut grounds: Vec<u32> = ring(anchor, size)
        .filter_map(|tile| map.component(tile))
        .collect();
    grounds.sort_unstable();
    grounds.dedup();
    grounds
}

/// The seat's built buildings worth guarding, most valuable first: its
/// Foundries, tech and production buildings, Extractors (more beside a
/// Foundry) and Reclaimers.
fn assets(observation: &ObservationData, frame: HomeFrame) -> Vec<Asset> {
    let foundries: Vec<&BuildingObs> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    let mut assets: Vec<Asset> = observation
        .my_buildings
        .iter()
        .filter(|building| building.built)
        .filter_map(|building| {
            let value = match building.kind {
                BuildingKind::Foundry if foundries.len() == 1 => 16,
                BuildingKind::Foundry => FOUNDRY_VALUE,
                BuildingKind::Crucible => 10,
                BuildingKind::Airworks => 9,
                BuildingKind::Fabricator => 8,
                BuildingKind::Extractor => {
                    let supported = foundries.iter().any(|foundry| {
                        gap(
                            foundry.anchor,
                            foundry.kind.base_stats().size,
                            building.anchor,
                            building.kind.base_stats().size,
                        ) <= 8
                    });
                    if supported { 8 } else { 6 }
                }
                BuildingKind::Reclaimer => 5,
                _ => return None,
            };
            Some(Asset {
                anchor: building.anchor,
                size: building.kind.base_stats().size,
                centre: footprint_centre(building.kind, building.anchor),
                value,
                ground: None,
                air: None,
            })
        })
        .collect();
    assets.sort_by_key(|asset| (Reverse(asset.value), frame.rank(frame.home, asset.centre)));
    assets
}

/// What one decision knows about where threats come from.
struct Known<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    memory: &'a Memory,
    frame: HomeFrame,
    /// Whether the seat has no army to speak of.
    exposed: bool,
    current: &'a [Enemy],
    /// What an approach falls back on, worked out when one first does, since
    /// many buildings share it: known enemy buildings and hostile starts,
    /// where remembered enemy aircraft were, and where remembered enemy
    /// ground units were beside each ground the seat's buildings stand by.
    bases: OnceCell<Vec<Base>>,
    aircraft: OnceCell<Vec<(i64, i64)>>,
    walkers: Vec<Walkers>,
}

/// Where remembered enemy ground units beside `grounds` were last seen.
struct Walkers {
    grounds: Vec<u32>,
    at: OnceCell<Vec<(i64, i64)>>,
}

impl Known<'_> {
    /// Where remembered enemies that fight or carry others in `domain` were
    /// last seen, in doubled coordinates: aircraft anywhere, ground units
    /// beside `grounds`.
    fn remembered(&self, domain: Domain, grounds: &[u32]) -> &[(i64, i64)] {
        let attackers = |keep: &dyn Fn(TilePos) -> bool| {
            self.memory
                .units()
                .iter()
                .filter(|unit| {
                    attacker(unit.kind) && unit.kind.stats().domain == domain && keep(unit.tile)
                })
                .map(|unit| doubled(unit.tile))
                .collect()
        };
        match domain {
            Domain::Air => self.aircraft.get_or_init(|| attackers(&|_| true)),
            Domain::Ground => self
                .walkers
                .iter()
                .find(|walkers| walkers.grounds == grounds)
                .map_or(&[], |walkers| {
                    walkers.at.get_or_init(|| {
                        attackers(&|tile| {
                            ring(tile, (1, 1)).any(|tile| {
                                self.map
                                    .component(tile)
                                    .is_some_and(|component| grounds.contains(&component))
                            })
                        })
                    })
                }),
        }
    }

    fn bases(&self) -> &[Base] {
        self.bases.get_or_init(|| {
            self.observation
                .enemy_buildings
                .iter()
                .map(|building| (building.kind, building.anchor))
                .chain(
                    self.map
                        .hostiles(self.observation.me)
                        .filter_map(|owner| self.map.start(owner))
                        .map(|anchor| (BuildingKind::Foundry, anchor)),
                )
                .map(|(kind, anchor)| Base {
                    centre: footprint_centre(kind, anchor),
                    grounds: grounds(self.map, anchor, kind.base_stats().size),
                })
                .collect()
        })
    }
}

/// A known enemy building or hostile start.
struct Base {
    centre: (i64, i64),
    grounds: Vec<u32>,
}

/// Where `asset` is attacked from in `domain`, and the samples along the
/// way: the nearest visible enemy that fights in that domain, else the
/// nearest remembered one, else for ground the nearest known enemy building
/// or hostile start, and for air the nearest known enemy Airworks. Enemy
/// ground units count only on or beside the asset's ground: across a chasm,
/// they arrive by air if at all, and land as a threat in sight. With no
/// enemy building or start on the asset's ground, one across a chasm counts
/// as a landing only while the seat is exposed, with no army to meet it. A
/// threat too close for any sample on the way is its own sample.
fn approach(known: &Known<'_>, asset: &Asset, domain: Domain) -> Option<Approach> {
    let Known {
        observation,
        map,
        frame,
        exposed,
        current,
        ..
    } = *known;
    let centre = asset.centre;
    let grounds = grounds(map, asset.anchor, asset.size);
    let beside = |others: &[u32]| others.iter().any(|component| grounds.contains(component));
    let nearest = |points: &mut dyn Iterator<Item = (i64, i64)>| {
        points.min_by_key(|point| (chebyshev(*point, centre), frame.rank(centre, *point)))
    };
    let current = nearest(
        &mut current
            .iter()
            .filter(|enemy| {
                enemy.domain == domain && (domain == Domain::Air || beside(&enemy.grounds))
            })
            .map(|enemy| enemy.at),
    );
    let remembered = || nearest(&mut known.remembered(domain, &grounds).iter().copied());
    let prior = || match domain {
        Domain::Ground => {
            let centres = |walk: bool| {
                known
                    .bases()
                    .iter()
                    .filter(move |base| !walk || beside(&base.grounds))
                    .map(|base| base.centre)
            };
            nearest(&mut centres(true))
                .map(|source| (source, Evidence::Prior))
                .or_else(|| {
                    exposed
                        .then(|| nearest(&mut centres(false)))
                        .flatten()
                        .map(|source| (source, Evidence::Landing))
                })
        }
        Domain::Air => nearest(
            &mut observation
                .enemy_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Airworks)
                .map(|building| footprint_centre(building.kind, building.anchor)),
        )
        .map(|source| (source, Evidence::Prior)),
    };
    let (source, evidence) = current
        .map(|source| (source, Evidence::Current))
        .or_else(|| remembered().map(|source| (source, Evidence::Remembered)))
        .or_else(prior)?;
    let mut samples = along(centre, source, &APPROACH);
    if samples.is_empty() {
        samples.push(source);
    }
    Some(Approach {
        samples,
        open: Vec::new(),
        spotted: Vec::new(),
        far: Vec::new(),
        evidence,
        source,
    })
}

/// Points `tiles` tiles from `centre` toward `source`, short of it, in
/// doubled coordinates. Integer division truncates toward zero, so a
/// mirrored seat's points mirror these.
fn along(centre: (i64, i64), source: (i64, i64), tiles: &[i64]) -> Vec<(i64, i64)> {
    let (dx, dy) = (source.0 - centre.0, source.1 - centre.1);
    let length = dx.abs().max(dy.abs());
    tiles
        .iter()
        .map(|tiles| 2 * tiles)
        .filter(|step| *step < length)
        .map(|step| (centre.0 + dx * step / length, centre.1 + dy * step / length))
        .collect()
}

/// A Foundry's minefield: a band about a blast wide either side of its
/// straight way in, from a few tiles out toward the threat, in doubled
/// coordinates.
struct Field {
    centre: (i64, i64),
    /// From the Foundry's centre to the threat.
    toward: (i64, i64),
}

impl Field {
    fn of(centre: (i64, i64), source: (i64, i64)) -> Self {
        Field {
            centre,
            toward: (source.0 - centre.0, source.1 - centre.1),
        }
    }

    /// Whether `point` lies ahead of the Foundry, short of the threat and
    /// within the band: among the tiles `first` tries, allowing for their
    /// rounding to tile centres.
    fn holds(&self, point: (i64, i64)) -> bool {
        let (dx, dy) = self.toward;
        let (px, py) = (point.0 - self.centre.0, point.1 - self.centre.1);
        let length = dx.abs().max(dy.abs());
        let squared = dx * dx + dy * dy;
        let slack = 2 * (dx.abs() + dy.abs());
        let ahead = px * dx + py * dy;
        let across = (px * dy - py * dx).abs();
        ahead > 0
            && ahead <= squared + slack
            && across * length <= 2 * MINEFIELD_WIDTH * squared + slack * length
    }

    /// The first tile that `fits`, a row at a time from nearest the Foundry
    /// outward, as its row and the tile; within a row, the least by `rank`.
    fn first<R: Ord>(
        &self,
        fits: impl Fn(TilePos) -> bool,
        rank: impl Fn(TilePos) -> R,
    ) -> Option<(i64, TilePos)> {
        let (dx, dy) = self.toward;
        let length = dx.abs().max(dy.abs());
        let mut row = MINEFIELD_START;
        while 2 * row < length {
            let (ax, ay) = (
                self.centre.0 + dx * 2 * row / length,
                self.centre.1 + dy * 2 * row / length,
            );
            let mut tiles: Vec<TilePos> = (-MINEFIELD_WIDTH..=MINEFIELD_WIDTH)
                .flat_map(|side| {
                    tiles_at((ax - dy * 2 * side / length, ay + dx * 2 * side / length))
                })
                .collect();
            tiles.sort_by_key(|tile| (rank(*tile), *tile));
            tiles.dedup();
            if let Some(tile) = tiles.into_iter().find(|tile| fits(*tile)) {
                return Some((row, tile));
            }
            row += 1;
        }
        None
    }
}

/// The tiles whose centres lie within half a tile of `point`: one when it
/// falls on a centre, up to four when it falls between them. Choosing among
/// them is left to the mirror-safe ranking.
fn tiles_at(point: (i64, i64)) -> Vec<TilePos> {
    let axis = |value: i64| {
        let half = value.div_euclid(2);
        if value.rem_euclid(2) == 1 {
            vec![half]
        } else {
            vec![half - 1, half]
        }
    };
    let ys = axis(point.1);
    axis(point.0)
        .into_iter()
        .flat_map(|x| ys.iter().map(move |y| TilePos::new(x as i32, *y as i32)))
        .collect()
}

/// Whether a Barricade at `tile` still lets the seat's home ground reach an
/// exit beside every own producer, every worked scrap node, and every
/// hostile start on that ground. One flood fill over the home ground, with
/// known buildings, known live scrap and the Barricade in the way.
pub(crate) fn keeps_paths(observation: &ObservationData, map: &MapModel, tile: TilePos) -> bool {
    let me = observation.me;
    let Some(start) = map.start(me) else {
        return false;
    };
    let Some(home) = map.component(start) else {
        return false;
    };
    let (width, height) = (observation.map_width, observation.map_height);
    let index = |tile: TilePos| {
        ((0..width).contains(&tile.x) && (0..height).contains(&tile.y))
            .then(|| (tile.y * width + tile.x) as usize)
    };
    let mut blocked = vec![false; (width * height) as usize];
    for building in observation
        .my_buildings
        .iter()
        .chain(&observation.ally_buildings)
        .chain(&observation.enemy_buildings)
    {
        let (w, h) = building.kind.base_stats().size;
        for footprint in (0..h).flat_map(|dy| (0..w).map(move |dx| building.anchor.offset(dx, dy)))
        {
            if let Some(at) = index(footprint) {
                blocked[at] = true;
            }
        }
    }
    for (node, amount) in &observation.known_scrap {
        if let Some(at) = index(*node)
            && *amount > 0
        {
            blocked[at] = true;
        }
    }
    if let Some(at) = index(tile) {
        blocked[at] = true;
    }
    let open = |tile: TilePos| {
        index(tile).is_some_and(|at| !blocked[at]) && map.component(tile) == Some(home)
    };
    let mut reached = vec![false; (width * height) as usize];
    let mut frontier: Vec<TilePos> = ring(start, BuildingKind::Foundry.base_stats().size)
        .filter(|tile| open(*tile))
        .collect();
    for tile in &frontier {
        if let Some(at) = index(*tile) {
            reached[at] = true;
        }
    }
    while let Some(tile) = frontier.pop() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = tile.offset(dx, dy);
            if let Some(at) = index(next)
                && !reached[at]
                && open(next)
            {
                reached[at] = true;
                frontier.push(next);
            }
        }
    }
    let beside = |anchor: TilePos, size: (i32, i32)| {
        ring(anchor, size).any(|tile| index(tile).is_some_and(|at| reached[at]))
    };
    let producers = observation
        .my_buildings
        .iter()
        .filter(|building| building.built && !building.kind.base_stats().produces.is_empty())
        .all(|building| beside(building.anchor, building.kind.base_stats().size));
    let nodes = map
        .home_nodes(me)
        .iter()
        .filter(|(node, _)| observation.known_scrap_at(*node))
        .all(|(node, _)| beside(*node, (1, 1)));
    let starts = map
        .hostiles(me)
        .filter_map(|owner| map.start(owner))
        .filter(|anchor| {
            ring(*anchor, BuildingKind::Foundry.base_stats().size)
                .any(|tile| map.component(tile) == Some(home))
        })
        .all(|anchor| beside(anchor, BuildingKind::Foundry.base_stats().size));
    producers && nodes && starts
}

fn distance2(a: (i64, i64), b: (i64, i64)) -> i64 {
    (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)
}

/// A footprint's extent, or a unit's centre, in doubled coordinates.
#[derive(Clone, Copy)]
struct Span {
    x: (i64, i64),
    y: (i64, i64),
}

impl Span {
    fn of(size: (i32, i32), anchor: TilePos) -> Self {
        let (x, y) = (2 * i64::from(anchor.x), 2 * i64::from(anchor.y));
        Span {
            x: (x, x + 2 * i64::from(size.0)),
            y: (y, y + 2 * i64::from(size.1)),
        }
    }

    /// Whether a Repair Bay over this span heals `other`: as the simulation
    /// measures, the straight distance between their nearest edges is within
    /// the aura's radius.
    fn aura(self, other: Span) -> bool {
        let apart = |a: (i64, i64), b: (i64, i64)| (a.0 - b.1).max(b.0 - a.1).max(0);
        let (dx, dy) = (apart(self.x, other.x), apart(self.y, other.y));
        let reach = (REPAIR_BAY_RADIUS + REPAIR_BAY_RADIUS).to_num::<i64>();
        dx * dx + dy * dy <= reach * reach
    }
}

/// Anchors for a `size` footprint standing off `asset` by the standoff
/// gaps.
fn sites_around(asset: &Asset, size: (i32, i32)) -> impl Iterator<Item = TilePos> + '_ {
    let low = -(STANDOFF[1] + size.0.max(size.1));
    let high = STANDOFF[1] + asset.size.0.max(asset.size.1);
    (low..=high)
        .flat_map(move |dy| (low..=high).map(move |dx| asset.anchor.offset(dx, dy)))
        .filter(move |tile| STANDOFF.contains(&gap(asset.anchor, asset.size, *tile, size)))
}

/// The domain a defense of `kind` fires at.
fn domain(kind: BuildingKind) -> Domain {
    match kind {
        BuildingKind::FlakTurret => Domain::Air,
        _ => Domain::Ground,
    }
}

fn chebyshev(a: (i64, i64), b: (i64, i64)) -> i64 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

fn points(worth: u64) -> u32 {
    u32::try_from(worth).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repair_bay_reaches_by_straight_distance_from_its_edges() {
        let bay = Span::of((2, 2), TilePos::new(10, 10));
        let unit = |x: i32, y: i32| {
            let (x, y) = doubled(TilePos::new(x, y));
            Span {
                x: (x, x),
                y: (y, y),
            }
        };
        assert!(
            bay.aura(unit(15, 10)),
            "three and a half tiles straight out"
        );
        assert!(!bay.aura(unit(16, 10)), "four and a half tiles out");
        assert!(
            !bay.aura(unit(15, 15)),
            "three tiles clear on both axes is over four tiles away"
        );
        let building = Span::of((2, 2), TilePos::new(16, 10));
        assert!(bay.aura(building), "edges four tiles apart");
        assert!(!bay.aura(Span::of((2, 2), TilePos::new(15, 15))));
    }
}
