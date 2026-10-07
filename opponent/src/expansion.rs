//! Where the seat could grow: expansion Foundries at scrap fields away from
//! every start, and Extractors on free frames. Values read the public map for
//! ground distance and starting scrap, and the seat's own knowledge for what is
//! left, what is claimed and what is dangerous.

use crate::frame::gap;
use crate::investments::Investment;
use crate::map::{MapModel, Site, UNREACHABLE};
use crate::memory::Memory;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, PlayerId};

/// Chebyshev reach around a site or frame that counts toward its danger.
const DANGER_REACH: i32 = 10;

/// A Foundry of the seat's own, or another seat's building, this close to a
/// site's nodes means someone already works it.
const HELD_REACH: i32 = 6;

/// Footprint of Foundries and Extractors, and so of sites and frames.
const WORKS: (i32, i32) = (2, 2);

/// An own Foundry this close to a frame supports its Extractor.
const SUPPORT_REACH: i32 = 8;

/// Expansion and Extractor investments with their scores. `greed` is the
/// 0..=100 trait; `depletion` is the per-mille share of home scrap mined.
pub(crate) fn candidates(
    observation: &ObservationData,
    map: &MapModel,
    memory: &Memory,
    greed: u8,
    depletion: u32,
) -> Vec<(Investment, u32)> {
    let mut list = Vec::new();
    for (index, site) in map.sites().iter().enumerate() {
        let Some(value) = value(observation, map, memory, site, greed) else {
            continue;
        };
        if value <= 0 {
            continue;
        }
        let score =
            (300 + 4 * i64::from(greed)) * value / 1_000 + 400 * i64::from(depletion) / 1_000;
        list.push((
            Investment::Expansion(u16::try_from(index).expect("site counts fit in u16")),
            u32::try_from(score.max(0)).unwrap_or(u32::MAX),
        ));
    }
    let home = map
        .start(observation.me)
        .and_then(|start| map.component(start));
    for frame in &observation.known_frames {
        if held(observation, *frame, -1)
            || memory.failed(BuildingKind::Extractor, *frame, observation.tick)
            || map.component(*frame) != home
            || contested(observation, *frame)
        {
            continue;
        }
        let supported = observation.my_buildings.iter().any(|building| {
            building.kind == BuildingKind::Foundry
                && building.built
                && gap(building.anchor, WORKS, *frame, WORKS) <= SUPPORT_REACH
        });
        let greed = u64::from(greed);
        let base = if supported {
            500 + 3 * greed
        } else {
            250 + 2 * greed
        };
        let score = base * 1_000 / (1_000 + danger(observation, memory, *frame));
        list.push((
            Investment::Extractor(*frame),
            u32::try_from(score).unwrap_or(u32::MAX),
        ));
    }
    list
}

/// The site's anchors on the seat's home ground, best first.
pub(crate) fn anchors(map: &MapModel, player: PlayerId, site: &Site) -> Vec<TilePos> {
    let home = map.start(player).and_then(|start| map.component(start));
    site.anchors
        .iter()
        .copied()
        .filter(|anchor| home.is_some() && map.component(*anchor) == home)
        .collect()
}

/// A site's value: what it would yield, weighted up by greed, less its
/// distance from home, how much nearer an enemy start is, and the danger
/// around it, weighted down by greed. `None` for a site the seat cannot or
/// should not take: with no anchor on its home ground, already held, or
/// failed at every such anchor.
pub(crate) fn value(
    observation: &ObservationData,
    map: &MapModel,
    memory: &Memory,
    site: &Site,
    greed: u8,
) -> Option<i64> {
    let me = observation.me;
    let reachable = anchors(map, me, site);
    let anchor = *reachable.first()?;
    if claimed(observation, site)
        || reachable
            .iter()
            .all(|anchor| memory.failed(BuildingKind::Foundry, *anchor, observation.tick))
    {
        return None;
    }
    let distance = map.distance(me, anchor);
    if distance == UNREACHABLE {
        return None;
    }
    let scrap: i64 = site
        .nodes
        .iter()
        .map(|(node, amount)| {
            if !observation.explored(*node) {
                return i64::from(*amount);
            }
            observation
                .known_scrap
                .binary_search_by_key(&(node.y, node.x), |(tile, _)| (tile.y, tile.x))
                .map_or(0, |index| i64::from(observation.known_scrap[index].1))
        })
        .sum();
    let frames = site
        .frames
        .iter()
        .filter(|frame| observation.known_frames.contains(frame) && !held(observation, **frame, -1))
        .count();
    let frames = i64::try_from(frames).expect("frame counts fit in i64");
    let resource = scrap.min(3_200) / 4 + 150 * frames;
    let distance = i64::from(distance);
    let contested = (distance - i64::from(map.hostile_distance(me, anchor)) + 100).max(0);
    let penalty = 8 * distance / 10
        + contested
        + i64::try_from(danger(observation, memory, anchor)).unwrap_or(i64::MAX);
    let weight = 750 + 5 * i64::from(greed);
    Some(resource * weight / 1_000 - penalty * (2_000 - weight) / 1_000)
}

/// Whether someone already works `site`: an own Foundry, or any other seat's
/// building, near its nodes.
fn claimed(observation: &ObservationData, site: &Site) -> bool {
    let own = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry);
    own.chain(&observation.ally_buildings)
        .chain(&observation.enemy_buildings)
        .any(|building| {
            let size = building.kind.base_stats().size;
            site.nodes
                .iter()
                .any(|(node, _)| gap(*node, (1, 1), building.anchor, size) <= HELD_REACH)
        })
}

/// Whether a known building stands within `reach` empty tiles of the
/// two-by-two footprint at `anchor`; a negative reach asks for overlap.
fn held(observation: &ObservationData, anchor: TilePos, reach: i32) -> bool {
    observation
        .my_buildings
        .iter()
        .chain(&observation.ally_buildings)
        .chain(&observation.enemy_buildings)
        .any(|building| {
            gap(
                anchor,
                WORKS,
                building.anchor,
                building.kind.base_stats().size,
            ) <= reach
        })
}

/// Whether an armed enemy in sight stands near the two-by-two footprint at
/// `anchor`: an Extractor lost there, or a new one, waits until it leaves.
fn contested(observation: &ObservationData, anchor: TilePos) -> bool {
    observation.enemy_units.iter().any(|enemy| {
        !enemy.kind.stats().weapons.is_empty()
            && gap(anchor, WORKS, enemy.tile, (1, 1)) <= DANGER_REACH
    })
}

/// Remembered enemy units near the two-by-two footprint at `anchor`, weighted
/// by confidence, plus known armed enemy buildings near it.
fn danger(observation: &ObservationData, memory: &Memory, anchor: TilePos) -> u64 {
    let now = observation.tick;
    let units: u64 = memory
        .units()
        .iter()
        .filter(|unit| {
            !unit.kind.stats().weapons.is_empty()
                && gap(anchor, WORKS, unit.tile, (1, 1)) <= DANGER_REACH
        })
        .map(|unit| unit.value(now))
        .sum();
    let buildings: u64 = observation
        .enemy_buildings
        .iter()
        .filter(|building| {
            !building.kind.base_stats().weapons.is_empty()
                && gap(
                    anchor,
                    WORKS,
                    building.anchor,
                    building.kind.base_stats().size,
                ) <= DANGER_REACH
        })
        .filter_map(|building| building.kind.base_stats().construction.as_ref())
        .map(|construction| u64::from(construction.cost))
        .sum();
    units + buildings
}
