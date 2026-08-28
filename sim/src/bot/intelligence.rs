//! Fog-honest strategic memory for the rules-based controller.
//!
//! The observation contains live enemy units and a mixture of live and ghost
//! buildings. This module turns those per-tick snapshots into stable contacts
//! without inventing information: darkness ages evidence, while current sight
//! over a remembered location is the only thing that invalidates it.

use super::observation::{OBSERVATION_VERSION, Observation};
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::stats::{BuildingKind, Domain, UnitKind, WeaponStats};
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;

/// Confidence is expressed in thousandths so strategic scoring remains
/// deterministic and free of presentation-only floating point.
pub const MAX_CONFIDENCE: u16 = 1_000;

/// A moving contact's old position stops being actionable quickly. The
/// contact itself remains until its last position is re-observed.
const UNIT_CONFIDENCE_HORIZON: Tick = 600;
/// Static structures remain useful intelligence much longer, but unseen
/// combat can still make an old ghost less trustworthy.
const BUILDING_CONFIDENCE_HORIZON: Tick = 3_600;

/// Whether a contact is justified by sight this tick or only by memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContactEvidence {
    /// The source appears in the current visible observation.
    Current,
    /// The source was seen before but is currently in fog.
    Remembered,
}

/// Last known state of a hostile unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitContact {
    /// Stable simulation id.
    pub id: UnitId,
    /// Owning player.
    pub player: PlayerId,
    /// Last observed unit kind.
    pub kind: UnitKind,
    /// Last observed tile.
    pub tile: TilePos,
    /// Last observed hit points.
    pub hp: u32,
    /// Tick on which this unit was last in current sight.
    pub last_seen: Tick,
    /// Current or remembered evidence.
    pub evidence: ContactEvidence,
}

impl UnitContact {
    /// Confidence in this unit still occupying its remembered area.
    ///
    /// Zero confidence does not mean the unit died. It means the old position
    /// is too stale to support a tactical commitment.
    pub fn confidence_at(&self, now: Tick) -> u16 {
        confidence(
            self.evidence,
            Some(self.last_seen),
            now,
            UNIT_CONFIDENCE_HORIZON,
        )
    }
}

/// Last known state of a hostile building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildingContact {
    /// Live id when this controller has personally seen the building. A ghost
    /// supplied without history has no targetable id contract.
    pub id: Option<BuildingId>,
    /// Owning player.
    pub player: PlayerId,
    /// Last observed kind.
    pub kind: BuildingKind,
    /// Stable footprint anchor.
    pub anchor: TilePos,
    /// Last observed hit points.
    pub hp: u32,
    /// Last observed construction state.
    pub built: bool,
    /// Last observed upgrade tier.
    pub tier: u8,
    /// Tick on which this building was last in current sight. This is unknown
    /// when the first snapshot supplied to the controller contains only a
    /// ghost.
    pub last_seen: Option<Tick>,
    /// Current or remembered evidence.
    pub evidence: ContactEvidence,
}

impl BuildingContact {
    /// Confidence that the remembered structure still exists as observed.
    /// A zero score preserves the contact but prevents stale intelligence from
    /// masquerading as a confirmed target.
    pub fn confidence_at(&self, now: Tick) -> u16 {
        confidence(
            self.evidence,
            self.last_seen,
            now,
            BUILDING_CONFIDENCE_HORIZON,
        )
    }
}

/// Identity and last-known position of one anti-air source covering a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AirDefenseSource {
    /// A mobile source. This includes dedicated anti-air, air-superiority
    /// fighters, and weaker dual-role weapons.
    Unit {
        /// Stable unit id.
        id: UnitId,
        /// Last observed kind.
        kind: UnitKind,
        /// Last observed tile.
        tile: TilePos,
    },
    /// A static source.
    Building {
        /// Targetable id, if the building has been seen live by this memory.
        id: Option<BuildingId>,
        /// Owner keeps same-anchor contacts from different opponents distinct.
        player: PlayerId,
        /// Last observed kind.
        kind: BuildingKind,
        /// Stable footprint anchor.
        anchor: TilePos,
    },
}

/// One known anti-air source whose observed weapon envelope covers a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirDefenseContact {
    /// Source identity and position.
    pub source: AirDefenseSource,
    /// Current or remembered evidence.
    pub evidence: ContactEvidence,
    /// Last confirmed sighting, if known.
    pub last_seen: Option<Tick>,
    /// Positional confidence in thousandths.
    pub confidence: u16,
    /// Nominal damage this source could apply per 100 ticks with the weapons
    /// that cover the queried tile. This is not omniscient combat prediction:
    /// line of sight, focus, cooldown phase, and intervening threats remain
    /// unknown.
    pub firepower_per_100_ticks: u32,
}

/// What the controller can honestly say about anti-air at a queried tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirDefenseEvidence {
    /// At least one covering source is currently visible.
    CurrentCoverage,
    /// Only remembered covering sources are known.
    RememberedCoverage,
    /// The target tile is visible and no known source covers it. This is local
    /// negative evidence, not proof that hidden mobile AA cannot arrive.
    VisibleWithoutKnownCoverage,
    /// The target is dark and no remembered covering source is actionable.
    Unknown,
}

/// Stable anti-air assessment at one target tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirDefenseAssessment {
    /// Queried target.
    pub target: TilePos,
    /// Whether the target tile itself is currently visible.
    pub target_visible: bool,
    /// Covering sources in canonical [`AirDefenseSource`] order.
    pub sources: Vec<AirDefenseContact>,
}

impl AirDefenseAssessment {
    /// Strongest evidence category represented by this assessment.
    pub fn evidence(&self) -> AirDefenseEvidence {
        if self
            .sources
            .iter()
            .any(|source| source.evidence == ContactEvidence::Current)
        {
            AirDefenseEvidence::CurrentCoverage
        } else if self.sources.iter().any(|source| source.confidence > 0) {
            AirDefenseEvidence::RememberedCoverage
        } else if self.target_visible {
            AirDefenseEvidence::VisibleWithoutKnownCoverage
        } else {
            AirDefenseEvidence::Unknown
        }
    }

    /// Number of currently visible mobile sources covering the target.
    pub fn current_mobile_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| {
                source.evidence == ContactEvidence::Current
                    && matches!(source.source, AirDefenseSource::Unit { .. })
            })
            .count()
    }

    /// Number of currently visible static sources covering the target.
    pub fn current_static_sources(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| {
                source.evidence == ContactEvidence::Current
                    && matches!(source.source, AirDefenseSource::Building { .. })
            })
            .count()
    }

    /// Confidence-weighted anti-air firepower. Remembered sources naturally
    /// fade without being falsely declared destroyed.
    pub fn weighted_firepower_per_100_ticks(&self) -> u32 {
        self.sources.iter().fold(0u32, |total, source| {
            let weighted = u64::from(source.firepower_per_100_ticks) * u64::from(source.confidence)
                / u64::from(MAX_CONFIDENCE);
            total.saturating_add(weighted.min(u64::from(u32::MAX)) as u32)
        })
    }
}

/// Controller-local, fog-honest memory of hostile assets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategicIntelligence {
    observed_at: Option<Tick>,
    map_width: i32,
    map_height: i32,
    visible: Vec<bool>,
    units: Vec<UnitContact>,
    buildings: Vec<BuildingContact>,
}

impl StrategicIntelligence {
    /// Creates empty intelligence for a new controller.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingests one observation from the controller's own point of view.
    ///
    /// Calls must be monotonic. Repeating the same tick is harmless and makes
    /// deterministic replay reconstruction straightforward.
    pub fn update(&mut self, observation: &Observation) {
        assert_eq!(
            observation.version, OBSERVATION_VERSION,
            "strategic intelligence requires the current observation schema"
        );
        assert!(
            self.observed_at
                .is_none_or(|previous| observation.tick >= previous),
            "strategic intelligence observations must be monotonic"
        );

        if self.observed_at.is_some()
            && (self.map_width != observation.map_width
                || self.map_height != observation.map_height)
        {
            self.units.clear();
            self.buildings.clear();
        }

        for contact in &mut self.units {
            contact.evidence = ContactEvidence::Remembered;
        }
        for contact in &mut self.buildings {
            contact.evidence = ContactEvidence::Remembered;
        }

        for unit in &observation.enemy_units {
            if let Some(contact) = self.units.iter_mut().find(|contact| contact.id == unit.id) {
                *contact = UnitContact {
                    id: unit.id,
                    player: unit.player,
                    kind: unit.kind,
                    tile: unit.tile,
                    hp: unit.hp,
                    last_seen: observation.tick,
                    evidence: ContactEvidence::Current,
                };
            } else {
                self.units.push(UnitContact {
                    id: unit.id,
                    player: unit.player,
                    kind: unit.kind,
                    tile: unit.tile,
                    hp: unit.hp,
                    last_seen: observation.tick,
                    evidence: ContactEvidence::Current,
                });
            }
        }
        self.units.retain(|contact| {
            contact.evidence == ContactEvidence::Current || !observation.visible(contact.tile)
        });
        self.units.sort_by_key(|contact| contact.id);

        for building in observation
            .enemy_buildings
            .iter()
            .filter(|building| building.seen)
        {
            if let Some(contact) = self.buildings.iter_mut().find(|contact| {
                contact.player == building.player && contact.anchor == building.anchor
            }) {
                *contact = BuildingContact {
                    id: Some(building.id),
                    player: building.player,
                    kind: building.kind,
                    anchor: building.anchor,
                    hp: building.hp,
                    built: building.built,
                    tier: building.tier,
                    last_seen: Some(observation.tick),
                    evidence: ContactEvidence::Current,
                };
            } else {
                self.buildings.push(BuildingContact {
                    id: Some(building.id),
                    player: building.player,
                    kind: building.kind,
                    anchor: building.anchor,
                    hp: building.hp,
                    built: building.built,
                    tier: building.tier,
                    last_seen: Some(observation.tick),
                    evidence: ContactEvidence::Current,
                });
            }
        }

        for ghost in observation
            .enemy_buildings
            .iter()
            .filter(|building| !building.seen)
        {
            if footprint_visible(observation, ghost.anchor, ghost.kind.base_stats().size) {
                continue;
            }
            if let Some(contact) = self
                .buildings
                .iter_mut()
                .find(|contact| contact.player == ghost.player && contact.anchor == ghost.anchor)
            {
                let same_kind = contact.kind == ghost.kind;
                contact.kind = ghost.kind;
                contact.hp = ghost.hp;
                contact.built = ghost.built;
                // Fog ghosts currently serialize at tier zero. Preserve a
                // tier this controller actually saw; a different structure
                // at the same anchor has no such continuity.
                if !same_kind {
                    contact.id = None;
                    contact.tier = ghost.tier;
                    contact.last_seen = None;
                } else if contact.last_seen.is_none() {
                    contact.tier = ghost.tier;
                }
            } else {
                self.buildings.push(BuildingContact {
                    id: None,
                    player: ghost.player,
                    kind: ghost.kind,
                    anchor: ghost.anchor,
                    hp: ghost.hp,
                    built: ghost.built,
                    tier: ghost.tier,
                    last_seen: None,
                    evidence: ContactEvidence::Remembered,
                });
            }
        }
        self.buildings.retain(|contact| {
            contact.evidence == ContactEvidence::Current
                || !footprint_visible(observation, contact.anchor, contact.kind.base_stats().size)
        });
        self.buildings.sort_by_key(|contact| {
            (
                contact.anchor.y,
                contact.anchor.x,
                contact.player,
                contact.kind,
            )
        });

        self.observed_at = Some(observation.tick);
        self.map_width = observation.map_width;
        self.map_height = observation.map_height;
        self.visible.clone_from(&observation.visible);
    }

    /// Tick of the most recently ingested observation.
    pub fn observed_at(&self) -> Option<Tick> {
        self.observed_at
    }

    /// Hostile unit contacts in stable id order.
    pub fn units(&self) -> &[UnitContact] {
        &self.units
    }

    /// Hostile building contacts in stable `(y, x, player, kind)` order.
    pub fn buildings(&self) -> &[BuildingContact] {
        &self.buildings
    }

    /// Assesses known anti-air whose observed weapon envelope covers `target`.
    ///
    /// A remembered mobile source is evidence of risk, not proof the unit
    /// remains in place. Conversely, a visible target without known coverage
    /// is only a locally observed vulnerability; hidden reinforcements remain
    /// unknown.
    pub fn air_defense_at(&self, target: TilePos) -> AirDefenseAssessment {
        let now = self.observed_at.unwrap_or(0);
        let mut sources = Vec::new();

        for contact in &self.units {
            let distance_sq = contact.tile.center().dist_sq(target.center());
            let firepower = air_firepower_covering(contact.kind.stats().weapons, distance_sq);
            if firepower == 0 {
                continue;
            }
            sources.push(AirDefenseContact {
                source: AirDefenseSource::Unit {
                    id: contact.id,
                    kind: contact.kind,
                    tile: contact.tile,
                },
                evidence: contact.evidence,
                last_seen: Some(contact.last_seen),
                confidence: contact.confidence_at(now),
                firepower_per_100_ticks: firepower,
            });
        }

        for contact in &self.buildings {
            let stats = contact.kind.tier_stats(contact.tier);
            let distance_sq = building_center(contact.anchor, stats.size).dist_sq(target.center());
            let firepower = air_firepower_covering(stats.weapons, distance_sq);
            if firepower == 0 {
                continue;
            }
            sources.push(AirDefenseContact {
                source: AirDefenseSource::Building {
                    id: contact.id,
                    player: contact.player,
                    kind: contact.kind,
                    anchor: contact.anchor,
                },
                evidence: contact.evidence,
                last_seen: contact.last_seen,
                confidence: contact.confidence_at(now),
                firepower_per_100_ticks: firepower,
            });
        }
        sources.sort_by_key(|source| source.source);

        AirDefenseAssessment {
            target,
            target_visible: self.visible(target),
            sources,
        }
    }

    fn visible(&self, tile: TilePos) -> bool {
        if tile.x < 0 || tile.y < 0 || tile.x >= self.map_width || tile.y >= self.map_height {
            return false;
        }
        let Ok(width) = usize::try_from(self.map_width) else {
            return false;
        };
        let Ok(x) = usize::try_from(tile.x) else {
            return false;
        };
        let Ok(y) = usize::try_from(tile.y) else {
            return false;
        };
        y.checked_mul(width)
            .and_then(|row| row.checked_add(x))
            .and_then(|index| self.visible.get(index))
            .copied()
            .unwrap_or(false)
    }
}

fn confidence(evidence: ContactEvidence, last_seen: Option<Tick>, now: Tick, horizon: Tick) -> u16 {
    if evidence == ContactEvidence::Current {
        return MAX_CONFIDENCE;
    }
    let Some(last_seen) = last_seen else {
        return 0;
    };
    let remaining = horizon.saturating_sub(now.saturating_sub(last_seen));
    ((remaining * u64::from(MAX_CONFIDENCE)) / horizon) as u16
}

fn footprint_visible(observation: &Observation, anchor: TilePos, size: (i32, i32)) -> bool {
    let (width, height) = size;
    (0..height).any(|dy| (0..width).any(|dx| observation.visible(anchor.offset(dx, dy))))
}

fn building_center(anchor: TilePos, size: (i32, i32)) -> Vec2Fx {
    let far = anchor.offset(size.0 - 1, size.1 - 1);
    (anchor.center() + far.center()) * HALF
}

fn air_firepower_covering(weapons: &[WeaponStats], distance_sq: Fx) -> u32 {
    weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Air))
        .filter(|weapon| {
            distance_sq <= weapon.range * weapon.range
                && distance_sq >= weapon.minimum_range * weapon.minimum_range
        })
        .fold(0u32, |total, weapon| {
            let firepower = u64::from(weapon.damage) * u64::from(weapon.salvo) * 100
                / u64::from(weapon.cooldown_ticks.max(1));
            total.saturating_add(firepower.max(1).min(u64::from(u32::MAX)) as u32)
        })
}

#[cfg(test)]
mod tests {
    use super::super::observation::{BuildingObs, UnitObs};
    use super::*;
    use crate::state::Faction;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn observation(tick: Tick) -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick,
            me: PlayerId(0),
            scrap: 0,
            map_width: 20,
            map_height: 12,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![false; 20 * 12],
            explored: vec![false; 20 * 12],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }

    fn set_visible(observation: &mut Observation, tile: TilePos) {
        let index = usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
        observation.visible[index] = true;
        observation.explored[index] = true;
    }

    fn enemy_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(1),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: false,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        }
    }

    fn enemy_building(id: u32, kind: BuildingKind, anchor: TilePos, seen: bool) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(1),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen,
            tier: 0,
        }
    }

    #[test]
    fn darkness_remembers_contacts_without_refreshing_them() {
        let flakhound_tile = TilePos::new(8, 6);
        let flak_anchor = TilePos::new(11, 6);
        let mut first = observation(100);
        set_visible(&mut first, flakhound_tile);
        set_visible(&mut first, flak_anchor);
        first.enemy_units = vec![enemy_unit(8, UnitKind::Flakhound, flakhound_tile)];
        first.enemy_buildings = vec![enemy_building(
            4,
            BuildingKind::FlakTurret,
            flak_anchor,
            true,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&first);

        let mut hidden = observation(200);
        hidden.enemy_buildings = vec![enemy_building(
            u32::MAX,
            BuildingKind::FlakTurret,
            flak_anchor,
            false,
        )];
        intelligence.update(&hidden);

        assert_eq!(intelligence.units().len(), 1);
        assert_eq!(
            intelligence.units()[0].evidence,
            ContactEvidence::Remembered
        );
        assert_eq!(intelligence.units()[0].last_seen, 100);
        assert!(intelligence.units()[0].confidence_at(200) < MAX_CONFIDENCE);
        assert_eq!(intelligence.buildings().len(), 1);
        assert_eq!(
            intelligence.buildings()[0].evidence,
            ContactEvidence::Remembered
        );
        assert_eq!(intelligence.buildings()[0].id, Some(BuildingId(4)));
        assert_eq!(intelligence.buildings()[0].last_seen, Some(100));
    }

    #[test]
    fn a_same_kind_ghost_preserves_the_tier_last_seen_in_current_sight() {
        let anchor = TilePos::new(11, 6);
        let mut first = observation(100);
        set_visible(&mut first, anchor);
        let mut turret = enemy_building(4, BuildingKind::Turret, anchor, true);
        turret.tier = 2;
        turret.hp = 700;
        first.enemy_buildings = vec![turret];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&first);

        let mut hidden = observation(200);
        let mut ghost = enemy_building(u32::MAX, BuildingKind::Turret, anchor, false);
        ghost.hp = 700;
        hidden.enemy_buildings = vec![ghost];
        intelligence.update(&hidden);

        let contact = &intelligence.buildings()[0];
        assert_eq!(contact.tier, 2);
        assert_eq!(contact.last_seen, Some(100));
        assert_eq!(contact.evidence, ContactEvidence::Remembered);
    }

    #[test]
    fn a_different_structure_at_the_same_anchor_does_not_inherit_the_old_tier() {
        let anchor = TilePos::new(11, 6);
        let mut first = observation(100);
        set_visible(&mut first, anchor);
        let mut turret = enemy_building(4, BuildingKind::Turret, anchor, true);
        turret.tier = 2;
        first.enemy_buildings = vec![turret];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&first);

        let mut hidden = observation(200);
        let mut replacement = enemy_building(u32::MAX, BuildingKind::FlakTurret, anchor, false);
        replacement.tier = 0;
        hidden.enemy_buildings = vec![replacement];
        intelligence.update(&hidden);

        let contact = &intelligence.buildings()[0];
        assert_eq!(contact.kind, BuildingKind::FlakTurret);
        assert_eq!(contact.tier, 0);
        assert_eq!(contact.id, None);
        assert_eq!(contact.last_seen, None);
        assert_eq!(contact.confidence_at(hidden.tick), 0);
        assert_eq!(contact.evidence, ContactEvidence::Remembered);
    }

    #[test]
    fn same_anchor_contacts_from_different_players_remain_distinct() {
        let anchor = TilePos::new(11, 6);
        let mut hidden = observation(100);
        let player_one = enemy_building(u32::MAX, BuildingKind::Turret, anchor, false);
        let mut player_two = enemy_building(u32::MAX, BuildingKind::FlakTurret, anchor, false);
        player_two.player = PlayerId(2);
        hidden.enemy_buildings = vec![player_two, player_one];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&hidden);

        assert_eq!(intelligence.buildings().len(), 2);
        assert_eq!(intelligence.buildings()[0].player, PlayerId(1));
        assert_eq!(intelligence.buildings()[1].player, PlayerId(2));
        assert!(
            intelligence
                .buildings()
                .iter()
                .all(|contact| contact.anchor == anchor)
        );
    }

    #[test]
    fn hidden_disappearance_does_not_claim_a_mobile_contact_is_gone() {
        let tile = TilePos::new(7, 5);
        let mut seen = observation(10);
        set_visible(&mut seen, tile);
        seen.enemy_units = vec![enemy_unit(2, UnitKind::Stinger, tile)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        intelligence.update(&observation(20));

        assert_eq!(intelligence.units().len(), 1);
        assert_eq!(intelligence.units()[0].id, UnitId(2));
        assert_eq!(
            intelligence.units()[0].evidence,
            ContactEvidence::Remembered
        );
    }

    #[test]
    fn current_sight_over_a_last_position_clears_mobile_memory() {
        let tile = TilePos::new(7, 5);
        let mut seen = observation(10);
        set_visible(&mut seen, tile);
        seen.enemy_units = vec![enemy_unit(2, UnitKind::Stinger, tile)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        intelligence.update(&observation(20));
        let mut clear = observation(30);
        set_visible(&mut clear, tile);
        intelligence.update(&clear);

        assert!(intelligence.units().is_empty());
    }

    #[test]
    fn a_moved_visible_unit_updates_instead_of_being_cleared_at_its_old_tile() {
        let old_tile = TilePos::new(7, 5);
        let new_tile = TilePos::new(9, 5);
        let mut first = observation(10);
        set_visible(&mut first, old_tile);
        first.enemy_units = vec![enemy_unit(2, UnitKind::Stinger, old_tile)];

        let mut moved = observation(20);
        set_visible(&mut moved, old_tile);
        set_visible(&mut moved, new_tile);
        moved.enemy_units = vec![enemy_unit(2, UnitKind::Stinger, new_tile)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&first);
        intelligence.update(&moved);

        assert_eq!(intelligence.units().len(), 1);
        assert_eq!(intelligence.units()[0].tile, new_tile);
        assert_eq!(intelligence.units()[0].last_seen, 20);
        assert_eq!(intelligence.units()[0].evidence, ContactEvidence::Current);
    }

    #[test]
    fn current_sight_over_any_remembered_footprint_tile_clears_the_building() {
        let anchor = TilePos::new(12, 7);
        let mut seen = observation(10);
        set_visible(&mut seen, anchor);
        seen.enemy_buildings = vec![enemy_building(5, BuildingKind::Foundry, anchor, true)];

        let mut hidden = observation(20);
        hidden.enemy_buildings = vec![enemy_building(
            u32::MAX,
            BuildingKind::Foundry,
            anchor,
            false,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        intelligence.update(&hidden);
        assert_eq!(intelligence.buildings().len(), 1);

        let mut clear = observation(30);
        set_visible(&mut clear, anchor.offset(1, 1));
        intelligence.update(&clear);

        assert!(intelligence.buildings().is_empty());
    }

    #[test]
    fn ghost_only_input_does_not_fabricate_a_fresh_sighting_or_live_id() {
        let anchor = TilePos::new(12, 7);
        let mut hidden = observation(500);
        hidden.enemy_buildings = vec![enemy_building(
            u32::MAX,
            BuildingKind::FlakTurret,
            anchor,
            false,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&hidden);

        let contact = &intelligence.buildings()[0];
        assert_eq!(contact.id, None);
        assert_eq!(contact.last_seen, None);
        assert_eq!(contact.confidence_at(500), 0);
        assert_eq!(contact.evidence, ContactEvidence::Remembered);
    }

    #[test]
    fn darkness_may_refresh_a_ghost_tier_but_visible_absence_wins_over_the_ghost() {
        let anchor = TilePos::new(12, 7);
        let mut first = observation(100);
        let mut ghost = enemy_building(u32::MAX, BuildingKind::Turret, anchor, false);
        ghost.tier = 1;
        first.enemy_buildings = vec![ghost.clone()];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&first);

        let mut hidden = observation(200);
        ghost.tier = 2;
        hidden.enemy_buildings = vec![ghost.clone()];
        intelligence.update(&hidden);
        assert_eq!(intelligence.buildings().len(), 1);
        let contact = &intelligence.buildings()[0];
        assert_eq!(contact.tier, 2);
        assert_eq!(contact.id, None);
        assert_eq!(contact.last_seen, None);
        assert_eq!(contact.evidence, ContactEvidence::Remembered);

        let mut visible_absence = observation(300);
        set_visible(&mut visible_absence, anchor);
        ghost.tier = 3;
        visible_absence.enemy_buildings = vec![ghost];
        intelligence.update(&visible_absence);

        assert!(
            intelligence.buildings().is_empty(),
            "current negative evidence over the footprint must clear memory even when the snapshot still carries a stale ghost"
        );
    }

    #[test]
    fn anti_air_assessment_includes_mobile_and_static_sources() {
        let target = TilePos::new(10, 6);
        let stinger_tile = TilePos::new(7, 6);
        let flak_anchor = TilePos::new(12, 6);
        let mut seen = observation(100);
        set_visible(&mut seen, target);
        set_visible(&mut seen, stinger_tile);
        set_visible(&mut seen, flak_anchor);
        seen.enemy_units = vec![enemy_unit(9, UnitKind::Stinger, stinger_tile)];
        seen.enemy_buildings = vec![enemy_building(
            3,
            BuildingKind::FlakTurret,
            flak_anchor,
            true,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        let assessment = intelligence.air_defense_at(target);

        assert_eq!(assessment.evidence(), AirDefenseEvidence::CurrentCoverage);
        assert_eq!(assessment.current_mobile_sources(), 1);
        assert_eq!(assessment.current_static_sources(), 1);
        assert_eq!(assessment.sources.len(), 2);
        assert!(assessment.weighted_firepower_per_100_ticks() > 0);
    }

    #[test]
    fn mobile_anti_air_covers_its_declared_range_boundary_but_not_beyond_it() {
        let source = TilePos::new(5, 6);
        let range_edge = TilePos::new(10, 6);
        let beyond_range = TilePos::new(11, 6);
        let mut seen = observation(100);
        set_visible(&mut seen, source);
        set_visible(&mut seen, range_edge);
        set_visible(&mut seen, beyond_range);
        seen.enemy_units = vec![enemy_unit(9, UnitKind::Flakhound, source)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);

        let edge = intelligence.air_defense_at(range_edge);
        assert_eq!(edge.current_mobile_sources(), 1);
        assert_eq!(edge.evidence(), AirDefenseEvidence::CurrentCoverage);

        let beyond = intelligence.air_defense_at(beyond_range);
        assert!(beyond.sources.is_empty());
        assert_eq!(
            beyond.evidence(),
            AirDefenseEvidence::VisibleWithoutKnownCoverage
        );
    }

    #[test]
    fn remembered_mobile_aa_is_risk_not_current_coverage() {
        let target = TilePos::new(10, 6);
        let stinger_tile = TilePos::new(7, 6);
        let mut seen = observation(100);
        set_visible(&mut seen, target);
        set_visible(&mut seen, stinger_tile);
        seen.enemy_units = vec![enemy_unit(9, UnitKind::Stinger, stinger_tile)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        intelligence.update(&observation(200));
        let assessment = intelligence.air_defense_at(target);

        assert_eq!(
            assessment.evidence(),
            AirDefenseEvidence::RememberedCoverage
        );
        assert_eq!(assessment.current_mobile_sources(), 0);
        assert_eq!(assessment.sources.len(), 1);
        assert!(assessment.sources[0].confidence < MAX_CONFIDENCE);
    }

    #[test]
    fn expired_positional_confidence_does_not_report_actionable_coverage() {
        let target = TilePos::new(10, 6);
        let stinger_tile = TilePos::new(7, 6);
        let mut seen = observation(100);
        set_visible(&mut seen, stinger_tile);
        seen.enemy_units = vec![enemy_unit(9, UnitKind::Stinger, stinger_tile)];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        intelligence.update(&observation(100 + UNIT_CONFIDENCE_HORIZON));
        let assessment = intelligence.air_defense_at(target);

        assert_eq!(
            assessment.sources.len(),
            1,
            "the sighting remains auditable"
        );
        assert_eq!(assessment.sources[0].confidence, 0);
        assert_eq!(assessment.weighted_firepower_per_100_ticks(), 0);
        assert_eq!(assessment.evidence(), AirDefenseEvidence::Unknown);
    }

    #[test]
    fn visible_negative_evidence_is_distinct_from_fog_unknown() {
        let target = TilePos::new(10, 6);
        let intelligence = {
            let mut value = StrategicIntelligence::new();
            value.update(&observation(10));
            value
        };
        assert_eq!(
            intelligence.air_defense_at(target).evidence(),
            AirDefenseEvidence::Unknown
        );

        let mut visible = observation(20);
        set_visible(&mut visible, target);
        let mut intelligence = intelligence;
        intelligence.update(&visible);
        assert_eq!(
            intelligence.air_defense_at(target).evidence(),
            AirDefenseEvidence::VisibleWithoutKnownCoverage
        );
    }

    #[test]
    fn hidden_counterfactuals_cannot_refresh_or_clear_contacts() {
        let tile = TilePos::new(8, 5);
        let anchor = TilePos::new(11, 6);
        let mut seen = observation(10);
        set_visible(&mut seen, tile);
        set_visible(&mut seen, anchor);
        seen.enemy_units = vec![enemy_unit(4, UnitKind::Flakhound, tile)];
        seen.enemy_buildings = vec![enemy_building(2, BuildingKind::FlakTurret, anchor, true)];

        let mut baseline = StrategicIntelligence::new();
        baseline.update(&seen);
        let mut counterfactual = baseline.clone();

        let mut one_hidden_world = observation(100);
        one_hidden_world.scrap = 10;
        one_hidden_world.known_scrap = vec![(TilePos::new(1, 1), 20)];
        one_hidden_world.enemy_buildings = vec![enemy_building(
            u32::MAX,
            BuildingKind::FlakTurret,
            anchor,
            false,
        )];
        let mut another_hidden_world = one_hidden_world.clone();
        another_hidden_world.scrap = 9_999;
        another_hidden_world.known_scrap = vec![(TilePos::new(19, 11), 800)];

        baseline.update(&one_hidden_world);
        counterfactual.update(&another_hidden_world);

        assert_eq!(baseline, counterfactual);
        assert_eq!(baseline.units()[0].last_seen, 10);
        assert_eq!(baseline.buildings()[0].last_seen, Some(10));
    }

    #[test]
    fn contacts_are_canonical_even_if_synthetic_input_is_not() {
        let mut seen = observation(10);
        seen.enemy_units = vec![
            enemy_unit(8, UnitKind::Flakhound, TilePos::new(8, 4)),
            enemy_unit(2, UnitKind::Stinger, TilePos::new(4, 4)),
        ];
        seen.enemy_buildings = vec![
            enemy_building(8, BuildingKind::FlakTurret, TilePos::new(14, 8), true),
            enemy_building(2, BuildingKind::Turret, TilePos::new(5, 3), true),
        ];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);

        assert_eq!(
            intelligence
                .units()
                .iter()
                .map(|contact| contact.id)
                .collect::<Vec<_>>(),
            vec![UnitId(2), UnitId(8)]
        );
        assert_eq!(
            intelligence
                .buildings()
                .iter()
                .map(|contact| contact.anchor)
                .collect::<Vec<_>>(),
            vec![TilePos::new(5, 3), TilePos::new(14, 8)]
        );
    }

    #[test]
    fn repeating_an_identical_tick_is_idempotent() {
        let unit_tile = TilePos::new(8, 4);
        let building_anchor = TilePos::new(12, 7);
        let mut seen = observation(100);
        set_visible(&mut seen, unit_tile);
        set_visible(&mut seen, building_anchor);
        seen.enemy_units = vec![enemy_unit(8, UnitKind::Flakhound, unit_tile)];
        seen.enemy_buildings = vec![enemy_building(
            4,
            BuildingKind::FlakTurret,
            building_anchor,
            true,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&seen);
        let once = intelligence.clone();
        intelligence.update(&seen);

        assert_eq!(intelligence, once);
    }

    #[test]
    fn changing_map_dimensions_discards_contacts_from_the_previous_map() {
        let old_unit = TilePos::new(19, 11);
        let old_building = TilePos::new(17, 10);
        let mut large = observation(100);
        set_visible(&mut large, old_unit);
        set_visible(&mut large, old_building);
        large.enemy_units = vec![enemy_unit(8, UnitKind::Flakhound, old_unit)];
        large.enemy_buildings = vec![enemy_building(
            4,
            BuildingKind::FlakTurret,
            old_building,
            true,
        )];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&large);

        let mut small = observation(200);
        small.map_width = 4;
        small.map_height = 3;
        small.visible = vec![false; 12];
        small.explored = vec![false; 12];
        intelligence.update(&small);

        assert!(intelligence.units().is_empty());
        assert!(intelligence.buildings().is_empty());
        assert_eq!(
            intelligence.air_defense_at(old_unit).evidence(),
            AirDefenseEvidence::Unknown
        );
    }

    #[test]
    fn an_unknown_observation_schema_is_rejected_without_mutating_memory() {
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation(100));
        let before = intelligence.clone();

        let mut unknown = observation(200);
        unknown.version = OBSERVATION_VERSION + 1;
        let result = catch_unwind(AssertUnwindSafe(|| intelligence.update(&unknown)));

        assert!(result.is_err());
        assert_eq!(intelligence, before);
    }

    #[test]
    #[should_panic(expected = "must be monotonic")]
    fn out_of_order_observations_are_rejected() {
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation(20));
        intelligence.update(&observation(19));
    }
}
