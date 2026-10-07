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
use crate::map::{Gate, MapModel, UNREACHABLE};
use crate::memory::{Memory, SeenUnit};
use crate::placement::{self, Layout};
use crate::profile::{PersonalityTraits, ResolvedProfile};
use crate::workers;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::stats::{
    BuildingStats, CHARGE_DAMAGE, Domain, FOUNDRY_REPAIR_PRICE, REPAIR_BAY_RADIUS, WeaponStats,
};
use oxide_sim::{BuildingKind, PlayerId, UnitKind};
use std::cell::OnceCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// What one of several bases is worth guarding; a lone one is worth more.
const FOUNDRY_VALUE: u64 = 12;

/// What an Extractor out on its own is worth guarding.
const OUTLYING_VALUE: u64 = 6;

/// Empty tiles between an Extractor and the nearest Foundry beyond which it
/// is guarded on its own rather than with that Foundry's base.
const OUTLYING_GAP: i32 = 8;

/// Tiles from a building's centre toward a threat sampled as its approach.
const APPROACH: [i64; 4] = [3, 6, 9, 12];

/// Tiles from a building's centre toward a threat an Array watches.
const FAR: [i64; 4] = [9, 12, 15, 18];

/// Tiles an Array's radar reaches.
const RADAR: i64 = 20;

/// Tiles beyond a base's edge along its approach where its Scuttle Charges
/// start.
const MINEFIELD_START: i64 = 3;

/// Tiles either side of a Foundry's straight way in that its Scuttle Charges
/// cover: about a blast wide.
const MINEFIELD_WIDTH: i64 = 2;

/// Empty tiles between a gun and the Barricade in front of it.
const BARRICADE_GAP: i32 = 1;

/// Divides an obstacle's worth into investment points.
const OBSTACLE_POINTS: u64 = 16;

/// Empty tiles between a building and a defense guarding it.
const STANDOFF: [i32; 2] = [2, 3];

/// Tiles on its home side inside which defenses holding a gate stand.
const GATE_DEPTH: u16 = 4;

/// Scrap of missing health a Repair Bay's aura must reach to be worth
/// building.
const BAY_WOUNDS: u64 = 300;

/// Divides a Repair Bay's worth into investment points.
const BAY_POINTS: u64 = 2;

/// Divides a defense's worth into investment points.
const POINTS: u64 = 48;

/// Scrap a gun's worth is quoted per, so scores per scrap stay on the scale
/// of the seat's other investments.
const QUOTE: u64 = 100;

/// Divides a gun's worth, the army scrap it holds off for each [`QUOTE`] of
/// its price, into investment points.
const GUN_POINTS: u64 = 5;

/// Divides a gun upgrade's worth, measured as a gun's, into investment
/// points.
const GUN_UPGRADE_POINTS: u64 = 5;

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

/// Per mille of a defense's worth an attack is assumed to bring when it means
/// to beat it, so guns worth a threat divided by it hold that threat off. It
/// models the attackers, not the seat, so it is the same at every difficulty:
/// the seat's own attack margin says how it attacks, not how others do.
const ATTACKER_MARGIN: u64 = 2_000;

/// What a seat's defenses must stand up to: at least an army at the stance's
/// minimum along any approach.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stakes {
    pub(crate) minimum: u64,
}

impl Stakes {
    pub(crate) fn of(profile: &ResolvedProfile) -> Self {
        Stakes {
            minimum: crate::missions::minimum(profile.stance),
        }
    }
}

#[cfg(test)]
impl Default for Stakes {
    /// A Balanced seat's.
    fn default() -> Self {
        Stakes {
            minimum: crate::missions::minimum(oxide_sim::scenario::BotStance::Balanced),
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
    /// How far the asset's buildings reach toward the threat beyond its
    /// Foundry or Extractor: a step along the straight way in, in doubled
    /// coordinates, measured as [`along`] measures; none at a cut.
    edge: i64,
    /// The cut the way in runs through, held instead of the base's edge.
    cut: Option<Held>,
    /// Points along the way in, beyond the edge or from each of the cut's
    /// gates, in doubled coordinates.
    samples: Vec<(i64, i64)>,
    /// At a cut, the gate each sample leads from.
    gate_of: Vec<usize>,
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
    /// The cover that holds the threat off at a sample, in army scrap.
    need: u64,
    /// The threat along the way, in army scrap, how its known enemies reach
    /// buildings, and whether they gather in clumps.
    threat: Threat,
}

/// A cut on the way in to the seat's start, held by guns on the home side of
/// each of its gates and charges across them.
struct Held {
    gates: Vec<Gate>,
    /// Ground distance from the seat's start to its near side, in tenths of
    /// a tile.
    distance: u16,
}

/// The value of the armed enemies the seat remembers near a source, at least
/// an army at the stance's minimum, their reach against buildings with each
/// one's value, and whether they gather in clumps a shell's splash hits
/// several of.
#[derive(Clone, Default)]
struct Threat {
    value: u64,
    reaches: Vec<(Fx, u64)>,
    clustered: bool,
}

impl Approach {
    /// The army scrap still missing at a sample `open` thousandths short.
    fn shortfall(&self, open: u64) -> u64 {
        self.need * open / 2_000
    }

    /// What `cover` holds off along this way in: its strength, less the
    /// share of the threat it cannot fire back at.
    fn held(&self, cover: &Cover) -> u64 {
        let unanswered: u64 = self
            .threat
            .reaches
            .iter()
            .filter(|(reach, _)| !cover.answers(*reach))
            .map(|(_, value)| value)
            .sum();
        let value = self.threat.value.max(1);
        cover.value[usize::from(self.threat.clustered)] * (value - unanswered.min(value)) / value
    }
}

/// What the seat guards: a base, grown from its Foundry, or an Extractor out
/// on its own.
struct Asset {
    anchor: TilePos,
    size: (i32, i32),
    centre: (i64, i64),
    value: u64,
    /// The buildings guarded together, as anchors and sizes, the Foundry or
    /// the Extractor first.
    members: Vec<(TilePos, (i32, i32))>,
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

/// Where a threat comes from, the domain it moves in, and, for ground units
/// that must walk, the grounds they count from.
type ThreatKey = ((i64, i64), Domain, Option<Vec<u32>>);

/// Where a weapon reaches, in doubled coordinates.
#[derive(Clone, Copy)]
struct Cover {
    centre: (i64, i64),
    /// What the weapon holds off, in army scrap, against a spread enemy and
    /// a clustered one: see [`strength`].
    value: [u64; 2],
    /// Squared doubled reach, and squared doubled minimum range.
    reach2: i64,
    min2: i64,
    /// Reach and minimum range, from the footprint centre.
    range: Fx,
    minimum: Fx,
    size: (i32, i32),
    ground: bool,
    air: bool,
}

impl Cover {
    fn of(kind: BuildingKind, tier: u8, anchor: TilePos) -> Option<Self> {
        let stats = kind.tier_stats(tier);
        let mut cover = Cover {
            centre: footprint_centre(kind, anchor),
            value: SPLASH_TARGETS.map(|targets| strength(stats, targets)),
            reach2: 0,
            min2: i64::MAX,
            range: Fx::ZERO,
            minimum: Fx::MAX,
            size: stats.size,
            ground: false,
            air: false,
        };
        for weapon in stats.weapons {
            let reach = (weapon.range + weapon.range).to_num::<i64>();
            let min = (weapon.minimum_range + weapon.minimum_range).to_num::<i64>();
            cover.reach2 = cover.reach2.max(reach * reach);
            cover.min2 = cover.min2.min(min * min);
            cover.range = cover.range.max(weapon.range);
            cover.minimum = cover.minimum.min(weapon.minimum_range);
            cover.ground |= weapon.targets.ground;
            cover.air |= weapon.targets.air;
        }
        (cover.ground || cover.air).then_some(cover)
    }

    /// Whether the gun fires back at an enemy of `reach` attacking it. The
    /// enemy measures to the footprint's nearest edge and the gun from its
    /// centre, so the gun must reach the enemy at a corner, and the enemy
    /// at a flat side must stand clear of the gun's minimum range.
    fn answers(self, reach: Fx) -> bool {
        let (width, height) = (i64::from(self.size.0), i64::from(self.size.1));
        let past = self.range - reach;
        // Four squared half-diagonals, against four squared reach to spare.
        let corner = Fx::from_num(width * width + height * height);
        let side = Fx::from_num(width.min(height)) / 2;
        past >= Fx::ZERO && past * past * 4 >= corner && side + reach >= self.minimum
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
    /// The seat's built Foundries, whose layouts keep their lanes clear.
    foundries: Vec<TilePos>,
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
        scope: Scope,
        settled: bool,
        exposed: bool,
        stakes: Stakes,
    ) -> Option<Self> {
        let frame = HomeFrame::of(observation, map)?;
        let foundries: Vec<TilePos> = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry && building.built)
            .map(|building| building.anchor)
            .collect();
        let covers: Vec<Cover> = observation
            .my_buildings
            .iter()
            .filter(|building| !building.provisional)
            .filter_map(|building| Cover::of(building.kind, building.tier, building.anchor))
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
        let mut assets = match scope {
            Scope::Bases => bases(observation, map, frame),
            Scope::Buildings => buildings(observation, frame),
        };
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
            gates: matches!(scope, Scope::Bases),
            current: &current,
            bases: OnceCell::new(),
            aircraft: OnceCell::new(),
            walkers,
        };
        // Many buildings share a threat's source.
        let mut threats: Vec<(ThreatKey, Threat)> = Vec::new();
        for asset in &mut assets {
            let centre = asset.centre;
            let around = grounds(map, asset.anchor, asset.size);
            asset.ground = approach(&known, asset, Domain::Ground);
            asset.air = approach(&known, asset, Domain::Air);
            for (domain, approach) in [
                (Domain::Ground, asset.ground.as_mut()),
                (Domain::Air, asset.air.as_mut()),
            ] {
                let Some(approach) = approach else {
                    continue;
                };
                // Ground units count only where ground connects them to the
                // building, unless they would come by landing.
                let beside = (domain == Domain::Ground && approach.evidence != Evidence::Landing)
                    .then_some(&around);
                let known = threats
                    .iter()
                    .find(|((source, of, on), _)| {
                        *source == approach.source && *of == domain && on.as_ref() == beside
                    })
                    .map(|(_, threat)| threat.clone());
                approach.threat = known.unwrap_or_else(|| {
                    let threat = threat(
                        memory,
                        map,
                        observation.tick,
                        (approach.source, domain),
                        beside.map(Vec::as_slice),
                        stakes,
                    );
                    threats.push(((approach.source, domain, beside.cloned()), threat.clone()));
                    threat
                });
                let need = approach.threat.value * 1_000 / ATTACKER_MARGIN;
                approach.need = need;
                let open = approach
                    .samples
                    .iter()
                    .map(|point| {
                        let covered: u64 = covers
                            .iter()
                            .filter(|cover: &&Cover| cover.covers(domain, *point))
                            .map(|cover| approach.held(cover))
                            .sum();
                        (2_000 * need.saturating_sub(covered))
                            .checked_div(need)
                            .unwrap_or(0)
                    })
                    .collect();
                approach.open = open;
                approach.spotted = approach.samples.iter().map(|point| seen(*point)).collect();
                let far = match &approach.cut {
                    Some(cut) => cut
                        .gates
                        .iter()
                        .flat_map(|gate| along(gate.centre, approach.source, 0, &FAR))
                        .collect(),
                    None => along(centre, approach.source, approach.edge, &FAR),
                };
                approach.far = far.into_iter().map(|point| (point, seen(point))).collect();
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
            foundries,
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

    /// What a defense of `kind` at `anchor` adds to `asset`'s approach: for a
    /// gun, the army scrap it closes of the shortfall against the threat
    /// along the way at each sample it covers, at most its strength. A Bastion counts only samples its owner's or an
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
            .map(|((_, open), _)| approach.shortfall(*open).min(approach.held(&cover)))
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

    /// The best spot for a Scuttle Charge: on the straight way in to a base or
    /// an Extractor out on its own whose field still falls short of its
    /// threat, nearest its edge. A field holds enough charges to deal the
    /// health of the threat along the way divided by the attacker margin,
    /// one body to a blast, in the share the guns covering the way leave
    /// open; every spot is worth the same until it does.
    fn charge(&self) -> Option<(TilePos, u64)> {
        let observation = self.observation;
        let charges: Vec<TilePos> = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::ScuttleCharge)
            .map(|building| building.anchor)
            .collect();
        let mut sites = Vec::new();
        for asset in &self.assets {
            let Some(approach) = asset.approach(Domain::Ground) else {
                continue;
            };
            if !self.usable(approach) {
                continue;
            }
            let health = health(self.memory, observation.tick, approach.source, self.stakes);
            // Charges deal the threat's health in the share of the way the
            // guns covering it leave open, of the samples `counts`, one body
            // to a blast.
            let need = |counts: &dyn Fn(usize) -> bool, share: (u64, u64)| {
                let open: Vec<u64> = approach
                    .open
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| counts(*index))
                    .map(|(_, open)| *open)
                    .collect();
                let open = open.iter().sum::<u64>() / open.len().max(1) as u64;
                (health * 1_000 / ATTACKER_MARGIN * open / 2_000 * share.0 / share.1.max(1))
                    .div_ceil(u64::from(CHARGE_DAMAGE))
            };
            let worth = asset.value * 2 * self.weight(approach, BuildingKind::ScuttleCharge);
            if let Some(cut) = &approach.cut {
                // Each gate holds its share of the threat by width, less what
                // the guns covering its own way on hold.
                let total = cut.gates.iter().map(|gate| gate.width() as u64).sum();
                let rank =
                    |tile: &TilePos| (self.frame.rank(approach.source, doubled(*tile)), *tile);
                let mut short: Vec<(Reverse<u64>, _, &Gate)> = cut
                    .gates
                    .iter()
                    .enumerate()
                    .filter_map(|(index, gate)| {
                        let laid = charges
                            .iter()
                            .filter(|charge| gate.tiles.contains(charge))
                            .count() as u64;
                        let need = need(
                            &|sample| approach.gate_of.get(sample) == Some(&index),
                            (gate.width() as u64, total),
                        );
                        (laid < need).then(|| {
                            (
                                Reverse(need - laid),
                                self.frame.rank(approach.source, gate.centre),
                                gate,
                            )
                        })
                    })
                    .collect();
                short.sort_by_key(|(short, rank, _)| (*short, *rank));
                let tile = short.into_iter().find_map(|(_, _, gate)| {
                    let mut tiles = gate.tiles.clone();
                    tiles.sort_by_key(rank);
                    tiles
                        .into_iter()
                        .find(|tile| self.placeable(BuildingKind::ScuttleCharge, *tile))
                });
                if let Some(tile) = tile {
                    sites.push((Reverse(worth), 0, rank(&tile).0, tile));
                }
                continue;
            }
            let field = Field::of(
                asset.centre,
                approach.source,
                approach.edge + 2 * MINEFIELD_START,
            );
            let laid = charges
                .iter()
                .filter(|charge| field.holds(doubled(**charge)))
                .count() as u64;
            if laid >= need(&|_| true, (1, 1)) {
                continue;
            }
            let Some((row, tile)) = field.first(
                |tile| self.placeable(BuildingKind::ScuttleCharge, tile),
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
        for (member, footprint) in self.assets.iter().flat_map(|asset| &asset.members) {
            // Every site around the building lies within this span.
            let span = Span::of(*footprint, *member);
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
            let at = (
                2 * i64::from(member.x) + i64::from(footprint.0),
                2 * i64::from(member.y) + i64::from(footprint.1),
            );
            for anchor in sites_around(*member, *footprint, size) {
                let bay = Span::of(size, anchor);
                let reached: u64 = near
                    .iter()
                    .filter(|(span, _)| bay.aura(*span))
                    .map(|(_, missing)| missing)
                    .sum();
                if reached >= BAY_WOUNDS {
                    let centre = footprint_centre(BuildingKind::RepairBay, anchor);
                    sites.push((reached, self.frame.rank(at, centre), anchor));
                }
            }
        }
        first_placeable(sites, |anchor| {
            self.placeable(BuildingKind::RepairBay, anchor)
        })
    }

    /// Whether `kind` may go at `anchor` as far as the seat knows, on ground
    /// a Harvester of the seat stands on, and was not refused there lately:
    /// off the lanes of the base's layout and the seat's gates, or for a
    /// buried Scuttle Charge, which blocks nothing, only on them, clear of the
    /// blocks' slots, unless no Foundry lays out its ground.
    fn placeable(&self, kind: BuildingKind, anchor: TilePos) -> bool {
        let crewed = self
            .map
            .component(anchor)
            .is_some_and(|ground| self.crews.contains(&ground));
        let (width, height) = kind.base_stats().size;
        let laned = || {
            (0..height)
                .flat_map(|dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
                .any(|tile| self.map.lane_of(&self.foundries, tile))
        };
        let me = self.observation.me;
        crewed
            && !self.memory.failed(kind, anchor, self.observation.tick)
            && if kind == BuildingKind::ScuttleCharge {
                self.map.gated(me, anchor) || !self.map.laid_out(&self.foundries, anchor) || laned()
            } else {
                !laned()
                    && (0..height)
                        .flat_map(|dy| (0..width).map(move |dx| anchor.offset(dx, dy)))
                        .all(|tile| !self.map.gated(me, tile))
            }
            && placement::check(self.observation, kind, anchor, &[], Layout::Apart).is_ok()
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
        let reach = Cover::of(kind, 0, TilePos::new(0, 0));
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
                    .map(|(open, _)| approach.shortfall(*open))
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
            for anchor in guard_sites(self.map, self.observation.me, asset, approach, kind) {
                let centre = footprint_centre(kind, anchor);
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
    let Some(guard) = Guard::new(
        observation,
        map,
        memory,
        Scope::Bases,
        settled,
        exposed,
        stakes,
    ) else {
        return Vec::new();
    };
    let fortification = u64::from(traits.fortification);
    let cunning = u64::midpoint(fortification, u64::from(traits.guile));
    let weights = [
        (BuildingKind::Turret, fortification),
        (BuildingKind::Bastion, fortification),
        (
            BuildingKind::FlakTurret,
            u64::midpoint(fortification, u64::from(traits.support)),
        ),
        (BuildingKind::Array, cunning / 2),
    ];
    let mut list: Vec<(Investment, u32)> = weights
        .into_iter()
        .filter_map(|(kind, weight)| {
            let (anchor, worth) = guard.best(kind)?;
            // Guns are weighed by what they hold off per scrap; an Array's
            // radar is not a gun.
            let score = match kind {
                BuildingKind::Array => worth * weight / POINTS,
                kind => worth * weight * QUOTE / (GUN_POINTS * price(kind.base_stats())),
            };
            Some((Investment::Defense { kind, anchor }, points(score)))
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
    let upgrade_weight = u64::midpoint(fortification, u64::from(traits.greed));
    list.extend(
        observation
            .my_buildings
            .iter()
            .filter_map(|building| upgrade(&guard, building, upgrade_weight)),
    );
    list
}

/// An upgrade for a built defense, worth what the strength it adds closes at
/// the approach samples the next tier covers, per scrap, or for an Array the
/// far points its radar watches, each by how sure the seat
/// is of the threat, unless an enemy in sight could hit it while it is down,
/// from the defense's reach or its own, or the next tier needs a building the
/// seat has not built.
fn upgrade(guard: &Guard<'_>, building: &BuildingObs, weight: u64) -> Option<(Investment, u32)> {
    let observation = guard.observation;
    if !building.built {
        return None;
    }
    let stats = building.kind.tier_stats(building.tier);
    let cover = Cover::of(building.kind, building.tier, building.anchor);
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
    // The next tier, where it reaches, and the strength it adds.
    let raised = cover.map(|cover| {
        let next = Cover::of(building.kind, building.tier + 1, building.anchor).unwrap_or(cover);
        (cover, next)
    });
    let worth: u64 = guard
        .assets
        .iter()
        .map(|asset| {
            if let Some((cover, next)) = raised {
                [Domain::Ground, Domain::Air]
                    .into_iter()
                    .filter_map(|domain| Some((domain, asset.approach(domain)?)))
                    .map(|(domain, approach)| {
                        // The army scrap of the shortfall at the samples the next
                        // tier covers that it closes beyond the gun today.
                        let short: u64 = approach
                            .samples
                            .iter()
                            .zip(&approach.open)
                            .map(|(point, open)| {
                                approach.shortfall(*open).min(raises(
                                    cover,
                                    next,
                                    domain,
                                    *point,
                                    |cover| approach.held(cover),
                                ))
                            })
                            .sum();
                        asset.value * short * approach.evidence.weight() / 1_000
                    })
                    .sum::<u64>()
            } else {
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
    // A gun's upgrade is weighed per scrap, as guns are; an Array's is not.
    let score = if cover.is_some() {
        worth * weight * QUOTE / (GUN_UPGRADE_POINTS * u64::from(next.cost).max(1))
    } else {
        worth * weight / UPGRADE_POINTS
    };
    (worth > 0).then(|| {
        (
            Investment::Upgrade {
                building: building.id,
                tier: building.tier + 1,
            },
            points(score),
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
    let Some(guard) = Guard::new(
        observation,
        map,
        memory,
        Scope::Buildings,
        true,
        true,
        stakes,
    ) else {
        return;
    };
    for (domain, kind) in [
        (Domain::Ground, BuildingKind::Turret),
        (Domain::Air, BuildingKind::FlakTurret),
    ] {
        let size = kind.base_stats().size;
        // An unfinished one beside its buildings already answers it.
        let unanswered = |asset: &Asset| {
            !observation.my_buildings.iter().any(|building| {
                building.kind == kind
                    && !building.built
                    && asset.members.iter().any(|(anchor, footprint)| {
                        gap(*anchor, *footprint, building.anchor, size) <= STANDOFF[1]
                    })
            })
        };
        let pressed = |asset: &Asset| {
            asset.approach(domain).is_some_and(|approach| {
                approach.evidence == Evidence::Current
                    && asset.members.iter().any(|(anchor, footprint)| {
                        let centre = (
                            2 * i64::from(anchor.x) + i64::from(footprint.0),
                            2 * i64::from(anchor.y) + i64::from(footprint.1),
                        );
                        chebyshev(approach.source, centre) <= 2 * EMERGENCY_TILES
                    })
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
            let Ok(allowed) =
                placement::check(observation, kind, anchor, ledger.planned(), Layout::Apart)
            else {
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

/// The armed enemies in `domain` the seat remembers near `source`, valued by
/// how sure it is they are still there, and at least an army at the stance's
/// minimum. Given `beside`, only those standing beside one of those grounds
/// count.
fn threat(
    memory: &Memory,
    map: &MapModel,
    now: u64,
    (source, domain): ((i64, i64), Domain),
    beside: Option<&[u32]>,
    stakes: Stakes,
) -> Threat {
    let units: Vec<&SeenUnit> = armed_near(memory, source, domain)
        .filter(|unit| {
            beside.is_none_or(|grounds| {
                ring(unit.tile, (1, 1)).any(|tile| {
                    map.component(tile)
                        .is_some_and(|component| grounds.contains(&component))
                })
            })
        })
        .collect();
    let reaches: Vec<(Fx, u64)> = units
        .iter()
        .map(|unit| (reach(unit.kind), unit.value(now)))
        .collect();
    let near: u64 = reaches.iter().map(|(_, value)| value).sum();
    Threat {
        value: near.max(stakes.minimum),
        reaches,
        clustered: composition::clustered(units),
    }
}

/// The health of the armed ground enemies the seat remembers near `source`,
/// each rounded up to whole Scuttle Charge blasts since a blast hits one
/// body, by how sure the seat is they are still there, and at least an army
/// of Sentinels at the stance's minimum.
fn health(memory: &Memory, now: u64, source: (i64, i64), stakes: Stakes) -> u64 {
    let near: u64 = armed_near(memory, source, Domain::Ground)
        .map(|unit| {
            let blasts = unit.kind.stats().max_hp.div_ceil(CHARGE_DAMAGE);
            u64::from(blasts * CHARGE_DAMAGE) * u64::from(unit.confidence(now)) / 1_000
        })
        .sum();
    let sentinel = UnitKind::Sentinel.stats();
    near.max(stakes.minimum * u64::from(sentinel.max_hp) / u64::from(sentinel.cost))
}

/// How far `kind` fires at buildings.
pub(crate) fn reach(kind: UnitKind) -> Fx {
    kind.stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.ground)
        .map(|weapon| weapon.range)
        .max()
        .unwrap_or(Fx::ZERO)
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

/// What a guard looks after.
#[derive(Clone, Copy)]
enum Scope {
    /// Bases and Extractors out on their own, defended in front of their
    /// edges.
    Bases,
    /// Each building on its own, as an attack on one is answered beside it.
    Buildings,
}

/// The seat's built buildings worth guarding each on its own, most valuable
/// first: its Foundries, tech and production buildings, Extractors (more
/// beside a Foundry) and Reclaimers.
fn buildings(observation: &ObservationData, frame: HomeFrame) -> Vec<Asset> {
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
            let size = building.kind.base_stats().size;
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
                            size,
                        ) <= OUTLYING_GAP
                    });
                    if supported { 8 } else { OUTLYING_VALUE }
                }
                BuildingKind::Reclaimer => 5,
                _ => return None,
            };
            Some(Asset {
                anchor: building.anchor,
                size,
                centre: footprint_centre(building.kind, building.anchor),
                value,
                members: vec![(building.anchor, size)],
                ground: None,
                air: None,
            })
        })
        .collect();
    assets.sort_by_key(|asset| (Reverse(asset.value), frame.rank(frame.home, asset.centre)));
    assets
}

/// What the seat guards, most valuable first: each base, and each Extractor
/// more than eight tiles from every Foundry on its own. A base grows from the
/// seat's start Foundry, a Foundry founded on an expansion site, or failing
/// those any Foundry on its ground, and holds every other built building but
/// defenses nearest it on that ground.
fn bases(observation: &ObservationData, map: &MapModel, frame: HomeFrame) -> Vec<Asset> {
    let size = |building: &BuildingObs| building.kind.base_stats().size;
    let foundries: Vec<&BuildingObs> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    let start = map.start(observation.me);
    let founded = |anchor: TilePos| {
        Some(anchor) == start
            || map
                .sites()
                .iter()
                .any(|site| site.anchors.contains(&anchor))
    };
    let mut origins: Vec<&BuildingObs> = foundries
        .iter()
        .copied()
        .filter(|foundry| founded(foundry.anchor))
        .collect();
    for foundry in &foundries {
        let ground = map.component(foundry.anchor);
        if !origins
            .iter()
            .any(|origin| map.component(origin.anchor) == ground)
        {
            origins.push(foundry);
        }
    }
    let value = if foundries.len() == 1 {
        16
    } else {
        FOUNDRY_VALUE
    };
    let mut assets: Vec<Asset> = origins
        .iter()
        .map(|origin| Asset {
            anchor: origin.anchor,
            size: size(origin),
            centre: footprint_centre(origin.kind, origin.anchor),
            value,
            members: vec![(origin.anchor, size(origin))],
            ground: None,
            air: None,
        })
        .collect();
    for building in observation.my_buildings.iter().filter(|building| {
        building.built
            && !defense(building.kind)
            && !origins.iter().any(|origin| origin.id == building.id)
    }) {
        let (anchor, footprint) = (building.anchor, size(building));
        let outlying = building.kind == BuildingKind::Extractor
            && !foundries.iter().any(|foundry| {
                gap(foundry.anchor, size(foundry), anchor, footprint) <= OUTLYING_GAP
            });
        if outlying {
            assets.push(Asset {
                anchor,
                size: footprint,
                centre: footprint_centre(building.kind, anchor),
                value: OUTLYING_VALUE,
                members: vec![(anchor, footprint)],
                ground: None,
                air: None,
            });
            continue;
        }
        let ground = map.component(anchor);
        let centre = footprint_centre(building.kind, anchor);
        let nearest = origins
            .iter()
            .zip(&mut assets)
            .filter(|(origin, _)| map.component(origin.anchor) == ground)
            .min_by_key(|(origin, base)| {
                (
                    gap(origin.anchor, size(origin), anchor, footprint),
                    frame.rank(centre, base.centre),
                )
            });
        if let Some((_, base)) = nearest {
            base.members.push((anchor, footprint));
        }
    }
    assets.sort_by_key(|asset| (Reverse(asset.value), frame.rank(frame.home, asset.centre)));
    assets
}

/// Whether `kind` is a defense the seat places to guard others, not one of
/// the buildings it guards.
fn defense(kind: BuildingKind) -> bool {
    matches!(
        kind,
        BuildingKind::Turret
            | BuildingKind::FlakTurret
            | BuildingKind::Bastion
            | BuildingKind::Array
            | BuildingKind::Barricade
            | BuildingKind::ScuttleCharge
            | BuildingKind::RepairBay
    )
}

/// What one decision knows about where threats come from.
struct Known<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    memory: &'a Memory,
    frame: HomeFrame,
    /// Whether the seat has no army to speak of.
    exposed: bool,
    /// Whether the start's base holds its ground way in at a gate.
    gates: bool,
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
    let cut = (known.gates
        && domain == Domain::Ground
        && evidence != Evidence::Landing
        && map.start(observation.me) == Some(asset.anchor))
    .then(|| held(known, asset, source))
    .flatten();
    let (edge, mut samples, gate_of): (i64, Vec<(i64, i64)>, Vec<usize>) = match &cut {
        // Each gate, and the way on from it.
        Some(cut) => {
            let (samples, gate_of) = cut
                .gates
                .iter()
                .enumerate()
                .flat_map(|(index, gate)| {
                    let mut samples = vec![gate.centre];
                    samples.extend(along(gate.centre, source, 0, &APPROACH));
                    samples.truncate(APPROACH.len());
                    samples.into_iter().map(move |sample| (sample, index))
                })
                .unzip();
            (0, samples, gate_of)
        }
        None => {
            let edge = edge(asset, source);
            (edge, along(centre, source, edge, &APPROACH), Vec::new())
        }
    };
    if samples.is_empty() {
        samples.push(source);
    }
    Some(Approach {
        edge,
        cut,
        samples,
        gate_of,
        open: Vec::new(),
        spotted: Vec::new(),
        far: Vec::new(),
        evidence,
        source,
        need: 0,
        threat: Threat::default(),
    })
}

/// The cut the start's base holds on its way in from `source`: the narrowest
/// beyond the reach of its buildings across the way to the hostile start
/// nearest `source`, while the threat stands beyond it, so that its way in
/// crosses it.
fn held(known: &Known<'_>, asset: &Asset, source: (i64, i64)) -> Option<Held> {
    let (map, me) = (known.map, known.observation.me);
    let reach = asset
        .members
        .iter()
        .flat_map(|(anchor, (width, height))| {
            (0..*height).flat_map(move |dy| (0..*width).map(move |dx| anchor.offset(dx, dy)))
        })
        .map(|tile| map.distance(me, tile))
        .filter(|distance| *distance != UNREACHABLE)
        .max()?;
    let hostile = map
        .hostiles(me)
        .filter_map(|hostile| {
            let centre = footprint_centre(BuildingKind::Foundry, map.start(hostile)?);
            Some((
                chebyshev(centre, source),
                known.frame.rank(source, centre),
                hostile,
            ))
        })
        .min_by_key(|(distance, rank, _)| (*distance, *rank))?
        .2;
    let cut = map.cut(me, hostile, reach.saturating_add(20))?;
    let at = TilePos::new(
        i32::try_from(source.0.div_euclid(2)).ok()?,
        i32::try_from(source.1.div_euclid(2)).ok()?,
    );
    (!cut.holds(at)).then(|| Held {
        gates: cut.gates.clone(),
        distance: cut.distance,
    })
}

/// Points `tiles` tiles beyond the step `edge` from `centre` toward
/// `source`, short of it, in doubled coordinates. Steps are measured along
/// the straight line by its larger axis. Integer division truncates toward
/// zero, so a mirrored seat's points mirror these.
fn along(centre: (i64, i64), source: (i64, i64), edge: i64, tiles: &[i64]) -> Vec<(i64, i64)> {
    let (dx, dy) = (source.0 - centre.0, source.1 - centre.1);
    let length = dx.abs().max(dy.abs());
    tiles
        .iter()
        .map(|tiles| edge + 2 * tiles)
        .filter(|step| *step < length)
        .map(|step| (centre.0 + dx * step / length, centre.1 + dy * step / length))
        .collect()
}

/// How far along the straight line from `centre` toward `source` `point`
/// lies: its projection, as a step [`along`] measures.
fn ahead(centre: (i64, i64), source: (i64, i64), point: (i64, i64)) -> i64 {
    let (dx, dy) = (source.0 - centre.0, source.1 - centre.1);
    let length = dx.abs().max(dy.abs());
    let squared = dx * dx + dy * dy;
    let dot = (point.0 - centre.0) * dx + (point.1 - centre.1) * dy;
    (dot * length).checked_div(squared).unwrap_or(0)
}

/// How far a footprint of `size` at `anchor` reaches along the straight line
/// from `centre` toward `source`: its furthest corner's projection.
fn furthest(centre: (i64, i64), source: (i64, i64), anchor: TilePos, size: (i32, i32)) -> i64 {
    let (x, y) = (2 * i64::from(anchor.x), 2 * i64::from(anchor.y));
    let (w, h) = (2 * i64::from(size.0), 2 * i64::from(size.1));
    [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
        .into_iter()
        .map(|corner| ahead(centre, source, corner))
        .max()
        .unwrap_or(0)
}

/// How far `asset`'s buildings besides its first reach toward `source`, as a
/// step [`along`] measures; none for an asset of one building.
fn edge(asset: &Asset, source: (i64, i64)) -> i64 {
    asset
        .members
        .iter()
        .skip(1)
        .map(|(anchor, size)| furthest(asset.centre, source, *anchor, *size))
        .max()
        .unwrap_or(0)
        .max(0)
}

/// A minefield: a band about a blast wide either side of an asset's straight
/// way in, from a step along it toward the threat, in doubled coordinates.
struct Field {
    centre: (i64, i64),
    /// From the asset's centre to the threat.
    toward: (i64, i64),
    /// The step along the way in where the field starts, as [`along`]
    /// measures.
    start: i64,
}

impl Field {
    fn of(centre: (i64, i64), source: (i64, i64), start: i64) -> Self {
        Field {
            centre,
            toward: (source.0 - centre.0, source.1 - centre.1),
            start,
        }
    }

    /// Whether `point` lies past the field's start, short of the threat and
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
            && ahead * length >= (self.start - 1) * squared
            && ahead <= squared + slack
            && across * length <= 2 * MINEFIELD_WIDTH * squared + slack * length
    }

    /// The first tile that `fits`, a row at a time from the field's start
    /// outward, as its step and the tile; within a row, the least by `rank`.
    fn first<R: Ord>(
        &self,
        fits: impl Fn(TilePos) -> bool,
        rank: impl Fn(TilePos) -> R,
    ) -> Option<(i64, TilePos)> {
        let (dx, dy) = self.toward;
        let length = dx.abs().max(dy.abs());
        let mut row = self.start;
        while row < length {
            let (ax, ay) = (
                self.centre.0 + dx * row / length,
                self.centre.1 + dy * row / length,
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
            row += 2;
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

/// Whether a `kind` at `anchor` still leaves its ground open: an exit beside
/// every own producer, every worked home scrap node and every hostile start
/// on that ground, a tile to work from beside itself and every own site
/// still going up, and no open ground beside it cut off, where units would
/// be trapped or trained into a pocket they never leave. One flood fill over
/// the ground from the rings of the seat's Foundries on it, with known
/// buildings, claims, `planned` footprints, known live scrap and the new
/// footprint in the way. A buried Scuttle Charge blocks nothing.
pub(crate) fn keeps_paths(
    observation: &ObservationData,
    map: &MapModel,
    kind: BuildingKind,
    anchor: TilePos,
    planned: &[(BuildingKind, TilePos)],
) -> bool {
    let me = observation.me;
    let Some(ground) = map.component(anchor) else {
        return false;
    };
    let (width, height) = (observation.map_width, observation.map_height);
    let index = |tile: TilePos| {
        ((0..width).contains(&tile.x) && (0..height).contains(&tile.y))
            .then(|| (tile.y * width + tile.x) as usize)
    };
    let tiles = |kind: BuildingKind, anchor: TilePos| {
        let (w, h) = kind.base_stats().size;
        (0..h).flat_map(move |dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
    };
    let claims: Vec<(BuildingKind, TilePos)> = observation
        .my_units
        .iter()
        .filter_map(|unit| unit.founding)
        .chain(planned.iter().copied())
        .filter(|(kind, _)| *kind != BuildingKind::ScuttleCharge)
        .collect();
    let mut blocked = vec![false; (width * height) as usize];
    let solid = observation
        .my_buildings
        .iter()
        .chain(&observation.ally_buildings)
        .chain(&observation.enemy_buildings)
        .filter(|building| building.kind != BuildingKind::ScuttleCharge)
        .map(|building| (building.kind, building.anchor))
        .chain(claims.iter().copied())
        .chain([(kind, anchor)]);
    for (kind, anchor) in solid {
        for tile in tiles(kind, anchor) {
            if let Some(at) = index(tile) {
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
    let open = |tile: TilePos| {
        index(tile).is_some_and(|at| !blocked[at]) && map.component(tile) == Some(ground)
    };
    let foundry = BuildingKind::Foundry.base_stats().size;
    let mut reached = vec![false; (width * height) as usize];
    let foundries: Vec<TilePos> = observation
        .my_buildings
        .iter()
        .filter(|building| {
            building.kind == BuildingKind::Foundry
                && building.built
                && map.component(building.anchor) == Some(ground)
        })
        .map(|building| building.anchor)
        .collect();
    // Ground no Foundry stands on has no exits to keep; a Foundry ringed
    // shut has none left.
    if foundries.is_empty() {
        return true;
    }
    let mut frontier: Vec<TilePos> = foundries
        .iter()
        .flat_map(|anchor| ring(*anchor, foundry))
        .filter(|tile| open(*tile))
        .collect();
    if frontier.is_empty() {
        return false;
    }
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
    let on = |tile: TilePos| map.component(tile) == Some(ground);
    let beside = |anchor: TilePos, size: (i32, i32)| {
        ring(anchor, size).any(|tile| index(tile).is_some_and(|at| reached[at]))
    };
    let reached_at = |tile: TilePos| index(tile).is_some_and(|at| reached[at]);
    // Workers build from a tile beside a footprint's edge, not its corner.
    let sides = |kind: BuildingKind, anchor: TilePos| {
        let (w, h) = kind.base_stats().size;
        let inside = |v: i32, low: i32, len: i32| (low..low + len).contains(&v);
        ring(anchor, (w, h))
            .filter(move |tile| inside(tile.x, anchor.x, w) || inside(tile.y, anchor.y, h))
    };
    let workable = |kind: BuildingKind, anchor: TilePos| sides(kind, anchor).any(reached_at);
    // Any pocket the footprint closes off borders one of its sides.
    let sealing = sides(kind, anchor).any(|tile| open(tile) && !reached_at(tile));
    let sites = observation
        .my_buildings
        .iter()
        .filter(|building| !building.built && building.kind != BuildingKind::ScuttleCharge)
        .map(|building| (building.kind, building.anchor))
        .chain(claims.iter().copied())
        .chain([(kind, anchor)])
        .filter(|(_, anchor)| on(*anchor))
        .all(|(kind, anchor)| workable(kind, anchor));
    let producers = observation
        .my_buildings
        .iter()
        .filter(|building| building.built && !building.kind.base_stats().produces.is_empty())
        .filter(|building| on(building.anchor))
        .all(|building| beside(building.anchor, building.kind.base_stats().size));
    let nodes = map
        .home_nodes(me)
        .iter()
        .filter(|(node, _)| observation.known_scrap_at(*node) && on(*node))
        .all(|(node, _)| beside(*node, (1, 1)));
    let starts = map
        .hostiles(me)
        .filter_map(|owner| map.start(owner))
        .filter(|anchor| ring(*anchor, foundry).any(on))
        .all(|anchor| beside(anchor, foundry));
    !sealing && sites && producers && nodes && starts
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

/// Anchors for a `size` footprint standing off a building of `footprint` at
/// `anchor` by the standoff gaps.
fn sites_around(
    anchor: TilePos,
    footprint: (i32, i32),
    size: (i32, i32),
) -> impl Iterator<Item = TilePos> {
    let low = -(STANDOFF[1] + size.0.max(size.1));
    let high = STANDOFF[1] + footprint.0.max(footprint.1);
    (low..=high)
        .flat_map(move |dy| (low..=high).map(move |dx| anchor.offset(dx, dy)))
        .filter(move |tile| STANDOFF.contains(&gap(anchor, footprint, *tile, size)))
}

/// Anchors for a `size` defense holding `gate` from its home side: within
/// [`GATE_DEPTH`] tiles short of it, off it.
fn gate_sites(
    map: &MapModel,
    me: PlayerId,
    gate: &Gate,
    distance: u16,
    size: (i32, i32),
) -> Vec<TilePos> {
    let (low, high) =
        gate.tiles
            .iter()
            .fold((gate.tiles[0], gate.tiles[0]), |(low, high), tile| {
                (
                    TilePos::new(low.x.min(tile.x), low.y.min(tile.y)),
                    TilePos::new(high.x.max(tile.x), high.y.max(tile.y)),
                )
            });
    let margin = i32::from(GATE_DEPTH) + size.0.max(size.1);
    let inside = distance.saturating_sub(10 * GATE_DEPTH)..distance;
    (low.y - margin..=high.y + margin)
        .flat_map(|y| (low.x - margin..=high.x + margin).map(move |x| TilePos::new(x, y)))
        .filter(|anchor| {
            (0..size.1)
                .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
                .all(|tile| inside.contains(&map.distance(me, tile)) && !map.gated(me, tile))
        })
        .collect()
}

/// Anchors for a defense of `kind` standing off one of `asset`'s buildings,
/// on the side `approach` comes from and no nearer than the asset's edge
/// along it, so a defense stands in front of a whole base rather than among
/// its buildings.
fn guard_sites(
    map: &MapModel,
    me: PlayerId,
    asset: &Asset,
    approach: &Approach,
    kind: BuildingKind,
) -> Vec<TilePos> {
    let size = kind.base_stats().size;
    if let Some(cut) = &approach.cut {
        let mut sites: Vec<TilePos> = cut
            .gates
            .iter()
            .flat_map(|gate| gate_sites(map, me, gate, cut.distance, size))
            .collect();
        sites.sort_unstable();
        sites.dedup();
        return sites;
    }
    // A site stands at most this far, as a step, beyond its building.
    let beyond = 3 * i64::from(STANDOFF[1] + size.0.max(size.1) + 1);
    let mut sites: Vec<TilePos> = asset
        .members
        .iter()
        .filter(|(anchor, footprint)| {
            furthest(asset.centre, approach.source, *anchor, *footprint) + beyond >= approach.edge
        })
        .flat_map(|(anchor, footprint)| sites_around(*anchor, *footprint, size))
        .filter(|anchor| {
            let step = ahead(
                asset.centre,
                approach.source,
                footprint_centre(kind, *anchor),
            );
            step > 0 && step >= approach.edge
        })
        .collect();
    sites.sort_unstable();
    sites.dedup();
    sites
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

/// How many enemies a splash shell hits, in thousandths, against a spread
/// enemy and a clustered one. A spread enemy stands too far apart for a
/// shell to hit two. Staged fights of a lone Bastion against Sentinels
/// matched about one and a third hits a shell in a column and four and a
/// half in a clump; both count as clustered, so a clustered enemy counts
/// three.
const SPLASH_TARGETS: [u64; 2] = [1_000, 3_000];

/// What a gun holds off, in army scrap: the price of the Sentinels that would
/// match it under Lanchester's square law, from the product of its damage per
/// tick and health against a Sentinel's, with a splash shell counting
/// `targets` thousandths of hits.
fn strength(stats: &BuildingStats, targets: u64) -> u64 {
    // Damage per tick, in thousandths.
    fn rate<'a>(weapons: impl Iterator<Item = &'a WeaponStats>, targets: u64) -> u64 {
        weapons
            .map(|weapon| {
                let hits = if weapon.splash.is_some() {
                    targets
                } else {
                    1_000
                };
                u64::from(weapon.damage) * u64::from(weapon.salvo.max(1)) * hits
                    / u64::from(weapon.cooldown_ticks.max(1))
            })
            .sum()
    }
    let sentinel = UnitKind::Sentinel.stats();
    let reference = rate(
        sentinel
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground),
        1_000,
    ) * u64::from(sentinel.max_hp);
    let gun = rate(stats.weapons.iter(), targets) * u64::from(stats.max_hp);
    let ratio = (gun * 1_000_000).checked_div(reference).unwrap_or(0);
    u64::from(sentinel.cost) * ratio.isqrt() / 1_000
}

/// What a gun's next tier `next` adds at `point` over the gun today `cover`,
/// each holding off what `held` says: all of it where the gun does not reach
/// yet, the difference where it does, none beyond the next tier's reach.
fn raises(
    cover: Cover,
    next: Cover,
    domain: Domain,
    point: (i64, i64),
    held: impl Fn(&Cover) -> u64,
) -> u64 {
    if !next.covers(domain, point) {
        0
    } else if cover.covers(domain, point) {
        held(&next).saturating_sub(held(&cover))
    } else {
        held(&next)
    }
}

/// What a building costs to place.
fn price(stats: &BuildingStats) -> u64 {
    stats
        .construction
        .as_ref()
        .map_or(QUOTE, |construction| u64::from(construction.cost).max(1))
}

fn points(worth: u64) -> u32 {
    u32::try_from(worth).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gun_holds_off_what_its_firepower_and_health_match_not_its_price() {
        let [spread, clumped] = SPLASH_TARGETS;
        let gun =
            |kind: BuildingKind, tier: u8, targets: u64| strength(kind.tier_stats(tier), targets);
        let turret = gun(BuildingKind::Turret, 0, spread);
        assert_eq!(
            turret,
            gun(BuildingKind::Turret, 0, clumped),
            "one hit a shot"
        );
        assert!(turret < gun(BuildingKind::Turret, 1, spread));
        assert!(gun(BuildingKind::Turret, 1, spread) < gun(BuildingKind::Turret, 2, spread));
        // Per scrap, a lone Bastion holds off fewer spread Sentinels than a
        // Turret, as staged fights found, and a clump lifts it.
        let per_scrap =
            |strength: u64, kind: BuildingKind| strength * 1_000 / price(kind.base_stats());
        let turret = per_scrap(turret, BuildingKind::Turret);
        let bastion = |targets| {
            per_scrap(
                gun(BuildingKind::Bastion, 0, targets),
                BuildingKind::Bastion,
            )
        };
        assert!(
            bastion(spread) < turret,
            "{} against {turret}",
            bastion(spread)
        );
        assert!(bastion(clumped) > bastion(spread) * 3 / 2);
    }

    #[test]
    fn a_gun_answers_only_enemies_it_can_fire_back_at() {
        let gun = |kind: BuildingKind| Cover::of(kind, 0, TilePos::new(10, 10)).unwrap();
        let answers = |kind: BuildingKind, enemy: UnitKind| gun(kind).answers(reach(enemy));
        assert!(answers(BuildingKind::Turret, UnitKind::Sentinel));
        assert!(
            !answers(BuildingKind::Turret, UnitKind::Lancer),
            "outranged"
        );
        assert!(!answers(BuildingKind::Turret, UnitKind::Bombard));
        assert!(answers(BuildingKind::Bastion, UnitKind::Sentinel));
        assert!(answers(BuildingKind::Bastion, UnitKind::Lancer));
        assert!(
            answers(BuildingKind::Bastion, UnitKind::Bombard),
            "even at a corner"
        );
        assert!(
            !answers(BuildingKind::Bastion, UnitKind::Scuttler),
            "a Scuttler bites from inside the minimum range"
        );
    }

    #[test]
    fn an_upgrade_adds_its_whole_strength_where_the_gun_did_not_reach() {
        let anchor = TilePos::new(10, 10);
        let turret = Cover::of(BuildingKind::Turret, 0, anchor).unwrap();
        let heavy = Cover::of(BuildingKind::Turret, 1, anchor).unwrap();
        let centre = footprint_centre(BuildingKind::Turret, anchor);
        // Two, five and a half and eight tiles out, in doubled coordinates.
        let near = (centre.0 + 4, centre.1);
        let edge = (centre.0 + 11, centre.1);
        let far = (centre.0 + 16, centre.1);
        assert!(turret.covers(Domain::Ground, near) && !turret.covers(Domain::Ground, edge));
        assert!(heavy.covers(Domain::Ground, edge) && !heavy.covers(Domain::Ground, far));
        let raised = |point| raises(turret, heavy, Domain::Ground, point, |cover| cover.value[0]);
        assert_eq!(raised(near), heavy.value[0] - turret.value[0]);
        assert_eq!(raised(edge), heavy.value[0]);
        assert_eq!(raised(far), 0);
    }

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
