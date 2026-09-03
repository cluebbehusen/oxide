use super::{BuilderObligation, BuilderResource, ProducerSlot, ProductionTiming, ResourceSnapshot};
use crate::ids::{BuildingId, UnitId};
use crate::stats::{QUEUE_CAP, UnitKind};
use chassis::grid::TilePos;

#[cfg(test)]
use super::{ProducerEgress, RecurringIncomeKind, next_multiple_at_or_after};
#[cfg(test)]
use crate::bot::observation::Observation;
#[cfg(test)]
use crate::stats::BuildingKind;
#[cfg(test)]
use chassis::Tick;

/// Strategic domain that owns a commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CommitmentDomain {
    /// Economy and territorial growth.
    Economy,
    /// Exact work owned by the strategic planners.
    Strategic,
    /// Unmigrated work adapted into the typed ledger.
    Legacy,
}

/// Stable identity of one proposal, obligation, or active plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CommitmentOwner {
    /// Strategic domain.
    pub(crate) domain: CommitmentDomain,
    /// Deterministic domain-local identity.
    pub(crate) sequence: u32,
}

impl CommitmentOwner {
    /// Creates an owner without assigning any implicit priority to its id.
    pub(crate) const fn new(domain: CommitmentDomain, sequence: u32) -> Self {
        Self { domain, sequence }
    }
}

/// A current-bank commitment request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrapClaim {
    /// Deduct irrevocable same-think spending.
    SpendNow(u32),
    /// Additive capital held by this owner until release or rollback.
    Hold(u32),
}

/// Role for an exact non-builder unit claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitClaimRole {
    /// Exact unit reserved by an unmigrated strategic planner.
    Strategic,
}

/// Exact use that currently owns a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactUnitUse {
    /// A non-builder unit role.
    Unit(UnitClaimRole),
    /// The exact builder for a construction claim.
    Builder {
        /// Work imported from the observation, or `None` for new work.
        obligation: Option<BuilderObligation>,
    },
}

/// A positive rectangular footprint with a top-left anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SiteFootprint {
    /// Top-left tile.
    anchor: TilePos,
    /// Width and height in tiles.
    size: (i32, i32),
}

impl SiteFootprint {
    /// Creates a footprint, rejecting zero or negative dimensions.
    pub(crate) const fn new(anchor: TilePos, size: (i32, i32)) -> Option<Self> {
        if size.0 > 0 && size.1 > 0 {
            Some(Self { anchor, size })
        } else {
            None
        }
    }

    fn overlaps(self, other: Self) -> bool {
        let self_left = i64::from(self.anchor.x);
        let self_top = i64::from(self.anchor.y);
        let self_right = self_left + i64::from(self.size.0);
        let self_bottom = self_top + i64::from(self.size.1);
        let other_left = i64::from(other.anchor.x);
        let other_top = i64::from(other.anchor.y);
        let other_right = other_left + i64::from(other.size.0);
        let other_bottom = other_top + i64::from(other.size.1);
        self_left < other_right
            && other_left < self_right
            && self_top < other_bottom
            && other_top < self_bottom
    }

    fn row_major_key(self) -> (i32, i32, i32, i32) {
        (self.anchor.y, self.anchor.x, self.size.1, self.size.0)
    }
}

/// One owner-attributed scrap total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnedScrap {
    /// Commitment owner.
    pub(crate) owner: CommitmentOwner,
    /// Current scrap committed.
    pub(crate) amount: u32,
}

/// One exact unit claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnedUnitClaim {
    /// Exact unit.
    pub(crate) unit: UnitId,
    /// Commitment owner.
    pub(crate) owner: CommitmentOwner,
    /// Exact role assigned by the owner.
    pub(crate) use_as: ExactUnitUse,
}

/// One exact site claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnedSiteClaim {
    /// Exact footprint.
    pub(crate) site: SiteFootprint,
    /// Commitment owner.
    pub(crate) owner: CommitmentOwner,
}

/// One exact producer-slot claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnedProducerClaim {
    /// Exact open queue position.
    pub(crate) slot: ProducerSlot,
    /// Unit appended at this exact position.
    pub(crate) kind: UnitKind,
    /// Earliest readiness and current egress evidence after prior queue work.
    pub(crate) timing: ProductionTiming,
    /// Commitment owner.
    pub(crate) owner: CommitmentOwner,
}

/// Why a requested resource could not be claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimConflict {
    /// The current bank cannot cover the additional commitment.
    InsufficientCurrentScrap {
        /// Additional bank capacity required.
        needed: u32,
        /// Uncommitted current bank.
        available: u32,
    },
    /// The id is not an observed own unit.
    UnknownUnit(UnitId),
    /// The unit cannot construct buildings or already owns observed work.
    BuilderUnavailable {
        /// Requested unit.
        unit: UnitId,
        /// Existing observed work, if that is what prevents the claim.
        obligation: Option<BuilderObligation>,
    },
    /// The caller tried to import different work than the observation reports.
    BuilderObligationMismatch {
        /// Requested unit.
        unit: UnitId,
        /// Work the caller intended to own.
        expected: BuilderObligation,
        /// Work actually visible in the observation.
        observed: Option<BuilderObligation>,
    },
    /// Another same-think claim already owns the exact unit.
    Unit {
        /// Requested unit.
        unit: UnitId,
        /// Existing claim.
        existing: OwnedUnitClaim,
    },
    /// Another same-think site overlaps the requested footprint.
    Site {
        /// Requested footprint.
        requested: SiteFootprint,
        /// First conflicting claim in canonical row-major order.
        existing: OwnedSiteClaim,
    },
    /// This queue position is not currently open on a completed producer.
    ProducerUnavailable(ProducerSlot),
    /// The completed producer cannot train the requested kind with current tech.
    ProducerCannotTrain {
        /// Exact producer.
        producer: BuildingId,
        /// Requested unit kind.
        kind: UnitKind,
    },
    /// Queue timing cannot be represented at this observation tick.
    ProducerTimingUnrepresentable {
        /// Exact producer.
        producer: BuildingId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct LedgerState {
    spent: Vec<OwnedScrap>,
    held: Vec<OwnedScrap>,
    units: Vec<OwnedUnitClaim>,
    sites: Vec<OwnedSiteClaim>,
    producers: Vec<OwnedProducerClaim>,
}

/// One-use snapshot of all mutable commitment state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LedgerCheckpoint {
    basis: ResourceSnapshot,
    state: LedgerState,
}

/// A checkpoint came from a ledger with different observed resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CheckpointMismatch;

/// What an owner released. Already-spent scrap is intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ReleaseSummary {
    /// Additive holds removed.
    pub(crate) held_scrap: u32,
    /// Exact unit claims removed, including builder claims.
    pub(crate) units: usize,
    /// Exact site claims removed.
    pub(crate) sites: usize,
    /// Exact producer claims removed.
    pub(crate) producers: usize,
}

/// Canonical same-think ownership of current scrap and exact resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitmentLedger {
    basis: ResourceSnapshot,
    state: LedgerState,
}

impl CommitmentLedger {
    /// Starts an empty ledger from one immutable resource snapshot.
    pub(crate) fn new(resources: &ResourceSnapshot) -> Self {
        Self {
            basis: resources.clone(),
            state: LedgerState::default(),
        }
    }

    /// Current bank not consumed by spending or additive holds.
    pub(crate) fn available_scrap(&self) -> u32 {
        self.basis
            .current_scrap
            .amount()
            .saturating_sub(self.committed_scrap())
    }

    /// Irrevocable spending recorded in this planning pass.
    pub(crate) fn spent_scrap(&self) -> u32 {
        sum_owned_scrap(&self.state.spent)
    }

    /// Additive held capital recorded in this planning pass.
    pub(crate) fn held_scrap(&self) -> u32 {
        sum_owned_scrap(&self.state.held)
    }

    /// Spent plus held scrap.
    pub(crate) fn committed_scrap(&self) -> u32 {
        self.spent_scrap().saturating_add(self.held_scrap())
    }

    /// Applies one current-bank claim without consulting forecast income.
    pub(crate) fn claim_scrap(
        &mut self,
        owner: CommitmentOwner,
        claim: ScrapClaim,
    ) -> Result<(), ClaimConflict> {
        match claim {
            ScrapClaim::SpendNow(amount) => {
                self.require_capacity(amount)?;
                add_owned_scrap(&mut self.state.spent, owner, amount);
            }
            ScrapClaim::Hold(amount) => {
                self.require_capacity(amount)?;
                add_owned_scrap(&mut self.state.held, owner, amount);
            }
        }
        Ok(())
    }

    /// Claims one exact observed own unit in a non-builder role.
    pub(crate) fn claim_unit(
        &mut self,
        owner: CommitmentOwner,
        unit: UnitId,
        role: UnitClaimRole,
    ) -> Result<(), ClaimConflict> {
        self.claim_exact_unit(owner, unit, ExactUnitUse::Unit(role), false)
    }

    /// Claims one exact free construction-capable unit.
    pub(crate) fn claim_builder(
        &mut self,
        owner: CommitmentOwner,
        unit: UnitId,
    ) -> Result<(), ClaimConflict> {
        self.claim_exact_unit(
            owner,
            unit,
            ExactUnitUse::Builder { obligation: None },
            true,
        )
    }

    /// Imports observed non-preemptible builder work under one explicit owner.
    ///
    /// This is the handoff for a legacy or persistent domain plan. It cannot
    /// manufacture an obligation or silently adopt a different one.
    pub(crate) fn import_builder_obligation(
        &mut self,
        owner: CommitmentOwner,
        unit: UnitId,
        expected: BuilderObligation,
    ) -> Result<(), ClaimConflict> {
        if self
            .basis
            .units
            .binary_search_by_key(&unit, |resource| resource.id)
            .is_err()
        {
            return Err(ClaimConflict::UnknownUnit(unit));
        }
        let observed = self
            .basis
            .builders
            .binary_search_by_key(&unit, |builder| builder.id)
            .ok()
            .map(|index| self.basis.builders[index].obligation);
        let Some(observed) = observed else {
            return Err(ClaimConflict::BuilderUnavailable {
                unit,
                obligation: None,
            });
        };
        if observed != Some(expected) {
            return Err(ClaimConflict::BuilderObligationMismatch {
                unit,
                expected,
                observed,
            });
        }
        self.insert_unit_claim(
            owner,
            unit,
            ExactUnitUse::Builder {
                obligation: Some(expected),
            },
        )
    }

    fn claim_exact_unit(
        &mut self,
        owner: CommitmentOwner,
        unit: UnitId,
        use_as: ExactUnitUse,
        builder: bool,
    ) -> Result<(), ClaimConflict> {
        if self
            .basis
            .units
            .binary_search_by_key(&unit, |resource| resource.id)
            .is_err()
        {
            return Err(ClaimConflict::UnknownUnit(unit));
        }
        let observed_builder = self
            .basis
            .builders
            .binary_search_by_key(&unit, |builder| builder.id)
            .ok()
            .map(|index| self.basis.builders[index]);
        if !builder
            && let Some(BuilderResource {
                obligation: Some(obligation),
                ..
            }) = observed_builder
        {
            return Err(ClaimConflict::BuilderUnavailable {
                unit,
                obligation: Some(obligation),
            });
        }
        if builder {
            match observed_builder {
                Some(BuilderResource {
                    obligation: None, ..
                }) => {}
                Some(builder) => {
                    return Err(ClaimConflict::BuilderUnavailable {
                        unit,
                        obligation: builder.obligation,
                    });
                }
                None => {
                    return Err(ClaimConflict::BuilderUnavailable {
                        unit,
                        obligation: None,
                    });
                }
            }
        }
        self.insert_unit_claim(owner, unit, use_as)
    }

    fn insert_unit_claim(
        &mut self,
        owner: CommitmentOwner,
        unit: UnitId,
        use_as: ExactUnitUse,
    ) -> Result<(), ClaimConflict> {
        match self
            .state
            .units
            .binary_search_by_key(&unit, |claim| claim.unit)
        {
            Ok(index) => Err(ClaimConflict::Unit {
                unit,
                existing: self.state.units[index],
            }),
            Err(index) => {
                self.state.units.insert(
                    index,
                    OwnedUnitClaim {
                        unit,
                        owner,
                        use_as,
                    },
                );
                Ok(())
            }
        }
    }

    /// Claims one exact positive footprint, rejecting any overlap.
    pub(crate) fn claim_site(
        &mut self,
        owner: CommitmentOwner,
        site: SiteFootprint,
    ) -> Result<(), ClaimConflict> {
        if let Some(existing) = self
            .state
            .sites
            .iter()
            .copied()
            .find(|existing| existing.site.overlaps(site))
        {
            return Err(ClaimConflict::Site {
                requested: site,
                existing,
            });
        }
        let key = site.row_major_key();
        let index = self
            .state
            .sites
            .binary_search_by_key(&key, |claim| claim.site.row_major_key())
            .unwrap_err();
        self.state
            .sites
            .insert(index, OwnedSiteClaim { site, owner });
        Ok(())
    }

    /// Appends one unit to the next contiguous position of a completed lane.
    pub(crate) fn append_production(
        &mut self,
        owner: CommitmentOwner,
        producer: BuildingId,
        kind: UnitKind,
    ) -> Result<OwnedProducerClaim, ClaimConflict> {
        let Some(lane) = self
            .basis
            .producers
            .iter()
            .find(|lane| lane.producer == producer)
        else {
            return Err(ClaimConflict::ProducerUnavailable(ProducerSlot {
                producer,
                queue_index: 0,
            }));
        };
        if !lane.trainable.contains(&kind) {
            return Err(ClaimConflict::ProducerCannotTrain { producer, kind });
        }
        let planned: Vec<_> = self
            .state
            .producers
            .iter()
            .filter(|claim| claim.slot.producer == producer)
            .map(|claim| claim.kind)
            .chain(std::iter::once(kind))
            .collect();
        let queue_index = lane.queued.len().saturating_add(planned.len() - 1);
        if queue_index >= QUEUE_CAP {
            return Err(ClaimConflict::ProducerUnavailable(ProducerSlot {
                producer,
                queue_index: u8::try_from(queue_index).unwrap_or(u8::MAX),
            }));
        }
        let timing = lane
            .production_timing(&planned)
            .ok_or(ClaimConflict::ProducerTimingUnrepresentable { producer })?;
        let slot = ProducerSlot {
            producer,
            queue_index: u8::try_from(queue_index).expect("queue capacity fits in u8"),
        };
        let claim = OwnedProducerClaim {
            slot,
            kind,
            timing,
            owner,
        };
        let index = match self
            .state
            .producers
            .binary_search_by_key(&slot, |claim| claim.slot)
        {
            Ok(_) => return Err(ClaimConflict::ProducerUnavailable(slot)),
            Err(index) => index,
        };
        self.state.producers.insert(index, claim);
        Ok(claim)
    }

    /// Releases this owner's revisable claims. Spending remains spent.
    pub(crate) fn release(&mut self, owner: CommitmentOwner) -> ReleaseSummary {
        let held_scrap = remove_owned_scrap(&mut self.state.held, owner);
        let units = retain_count(&mut self.state.units, |claim| claim.owner != owner);
        let sites = retain_count(&mut self.state.sites, |claim| claim.owner != owner);
        let producers = retain_count(&mut self.state.producers, |claim| claim.owner != owner);
        for lane in &self.basis.producers {
            let mut planned = Vec::new();
            for claim in self
                .state
                .producers
                .iter_mut()
                .filter(|claim| claim.slot.producer == lane.producer)
            {
                planned.push(claim.kind);
                claim.slot.queue_index =
                    u8::try_from(lane.queued.len().saturating_add(planned.len() - 1))
                        .expect("queue capacity fits in u8");
                claim.timing = lane
                    .production_timing(&planned)
                    .expect("removing appends preserves representable legal timing");
            }
        }
        ReleaseSummary {
            held_scrap,
            units,
            sites,
            producers,
        }
    }

    /// Captures all mutable claims for speculative rollback.
    pub(crate) fn checkpoint(&self) -> LedgerCheckpoint {
        LedgerCheckpoint {
            basis: self.basis.clone(),
            state: self.state.clone(),
        }
    }

    /// Restores a checkpoint made from the same observed resource basis.
    pub(crate) fn rollback(
        &mut self,
        checkpoint: LedgerCheckpoint,
    ) -> Result<(), CheckpointMismatch> {
        if self.basis != checkpoint.basis {
            return Err(CheckpointMismatch);
        }
        self.state = checkpoint.state;
        Ok(())
    }

    /// Irrevocable spend rows in owner order.
    #[cfg(test)]
    pub(crate) fn spending(&self) -> &[OwnedScrap] {
        &self.state.spent
    }

    /// Additive hold rows in owner order.
    pub(crate) fn holds(&self) -> &[OwnedScrap] {
        &self.state.held
    }

    /// Exact unit claims in unit-id order.
    #[cfg(test)]
    pub(crate) fn unit_claims(&self) -> &[OwnedUnitClaim] {
        &self.state.units
    }

    /// Exact site claims in explicit row-major order.
    #[cfg(test)]
    pub(crate) fn site_claims(&self) -> &[OwnedSiteClaim] {
        &self.state.sites
    }

    /// Exact producer claims in producer-id then queue-index order.
    #[cfg(test)]
    pub(crate) fn producer_claims(&self) -> &[OwnedProducerClaim] {
        &self.state.producers
    }

    fn require_capacity(&self, needed: u32) -> Result<(), ClaimConflict> {
        let available = self.available_scrap();
        if needed > available {
            Err(ClaimConflict::InsufficientCurrentScrap { needed, available })
        } else {
            Ok(())
        }
    }
}

fn sum_owned_scrap(rows: &[OwnedScrap]) -> u32 {
    rows.iter()
        .map(|claim| claim.amount)
        .fold(0, u32::saturating_add)
}

fn add_owned_scrap(rows: &mut Vec<OwnedScrap>, owner: CommitmentOwner, amount: u32) {
    if amount == 0 {
        return;
    }
    match rows.binary_search_by_key(&owner, |claim| claim.owner) {
        Ok(index) => rows[index].amount = rows[index].amount.saturating_add(amount),
        Err(index) => rows.insert(index, OwnedScrap { owner, amount }),
    }
}

fn remove_owned_scrap(rows: &mut Vec<OwnedScrap>, owner: CommitmentOwner) -> u32 {
    rows.binary_search_by_key(&owner, |claim| claim.owner)
        .ok()
        .map(|index| rows.remove(index).amount)
        .unwrap_or(0)
}

fn retain_count<T>(rows: &mut Vec<T>, mut keep: impl FnMut(&T) -> bool) -> usize {
    let before = rows.len();
    rows.retain(|row| keep(row));
    before - rows.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::command::{Command, PlayerCommand};
    use crate::event::Event;
    use crate::ids::PlayerId;
    use crate::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use crate::state::{ExtractorIncome, Faction};

    const ME: PlayerId = PlayerId(0);

    fn owner(domain: CommitmentDomain, sequence: u32) -> CommitmentOwner {
        CommitmentOwner::new(domain, sequence)
    }

    fn unit(id: u32, kind: UnitKind) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: ME,
            kind,
            tile: TilePos::new(id as i32, 1),
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn building(id: u32, kind: BuildingKind, anchor: TilePos, built: bool) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: ME,
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built,
            seen: true,
            tier: 0,
        }
    }

    fn observation(scrap: u32) -> Observation {
        Observation {
            me: ME,
            scrap,
            faction: Faction::Ferrous,
            map_width: 80,
            map_height: 60,
            visible: vec![true; 80 * 60],
            explored: vec![true; 80 * 60],
            ..Observation::default()
        }
    }

    fn scenario_players(scrap: u32) -> Vec<PlayerSpec> {
        vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap,
                bot: false,
                bot_config: None,
            },
        ]
    }

    fn bordered_ground(width: usize, height: usize) -> Vec<Vec<char>> {
        let mut tiles = vec![vec!['.'; width]; height];
        tiles[0].fill('#');
        tiles[height - 1].fill('#');
        for row in &mut tiles {
            row[0] = '#';
            row[width - 1] = '#';
        }
        tiles
    }

    fn map_rows(tiles: Vec<Vec<char>>) -> Vec<String> {
        tiles
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect()
    }

    fn income_parity_state(
        support_owner: u8,
        support_anchor: TilePos,
        support_built: bool,
    ) -> crate::State {
        let extractor = TilePos::new(20, 5);
        let mut tiles = bordered_ground(42, 22);
        tiles[1][1] = '1';
        tiles[18][38] = '2';
        tiles[extractor.y as usize][extractor.x as usize] = 'E';
        let scenario = crate::Scenario {
            name: "resource-forecast-parity".into(),
            seed: 17,
            map: map_rows(tiles),
            players: scenario_players(0),
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5,
                y: 5,
            }],
            buildings: vec![
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Extractor,
                    x: extractor.x,
                    y: extractor.y,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Reclaimer,
                    x: 4,
                    y: 14,
                },
                BuildingSpec {
                    player: support_owner,
                    kind: BuildingKind::Foundry,
                    x: support_anchor.x,
                    y: support_anchor.y,
                },
            ],
            meta: None,
        };
        let mut state = scenario.build().expect("forecast parity scenario builds");
        if !support_built {
            state
                .buildings
                .iter_mut()
                .find(|building| {
                    building.player == ME
                        && building.kind == BuildingKind::Foundry
                        && building.anchor == support_anchor
                })
                .expect("the own support candidate stands")
                .built = false;
        }
        state.tick = crate::stats::FOUNDRY_DRIP_START_TICK - 2;
        state
    }

    fn blocked_egress_state() -> (crate::State, BuildingId, BuildingId) {
        let producer_anchor = TilePos::new(11, 9);
        let door = TilePos::new(10, 9);
        let mut tiles = bordered_ground(26, 20);
        tiles[1][1] = '1';
        tiles[16][22] = '2';
        for tile in crate::tick::rect_adjacent_tiles(
            producer_anchor,
            BuildingKind::Fabricator.base_stats().size,
        ) {
            if tile != door {
                tiles[tile.y as usize][tile.x as usize] = '#';
            }
        }
        let scenario = crate::Scenario {
            name: "blocked-producer-egress".into(),
            seed: 23,
            map: map_rows(tiles),
            players: scenario_players(1_000),
            units: Vec::new(),
            buildings: vec![
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Fabricator,
                    x: producer_anchor.x,
                    y: producer_anchor.y,
                },
                BuildingSpec {
                    player: 1,
                    kind: BuildingKind::Turret,
                    x: door.x,
                    y: door.y,
                },
            ],
            meta: None,
        };
        let mut state = scenario.build().expect("blocked-egress scenario builds");
        let producer = state
            .buildings()
            .iter()
            .find(|building| building.player == ME && building.kind == BuildingKind::Fabricator)
            .expect("the Fabricator stands")
            .id;
        let blocker = state
            .buildings()
            .iter()
            .find(|building| building.player != ME && building.kind == BuildingKind::Turret)
            .expect("the hostile doorstep blocker stands")
            .id;
        let building = state.building_mut(producer).unwrap();
        building.queue.push_back(UnitKind::Lancer);
        building.progress = UnitKind::Lancer.stats().train_ticks - 1;
        (state, producer, blocker)
    }

    fn resource_observation(scrap: u32) -> Observation {
        let mut obs = observation(scrap);
        obs.my_units = vec![
            unit(3, UnitKind::Sentinel),
            unit(1, UnitKind::Harvester),
            unit(2, UnitKind::Excavator),
        ];
        obs.my_buildings = vec![
            building(8, BuildingKind::Foundry, TilePos::new(3, 3), true),
            building(4, BuildingKind::Fabricator, TilePos::new(8, 3), true),
        ];
        obs.my_queues = vec![vec![UnitKind::Sentinel], Vec::new()];
        obs
    }

    fn run_until_trained(state: &mut crate::State, producer: BuildingId, kind: UnitKind) -> Tick {
        let command = PlayerCommand {
            player: ME,
            command: Command::Train {
                building: producer,
                kind,
            },
        };
        let mut next = Some(command);
        loop {
            let commands = next.as_slice();
            let report = state.tick(commands);
            next = None;
            if report.events.iter().any(|event| {
                matches!(
                    event,
                    Event::UnitTrained {
                        building,
                        kind: trained,
                        ..
                    } if *building == producer && *trained == kind
                )
            }) {
                return report.tick;
            }
        }
    }

    #[test]
    fn current_bank_and_completed_income_forecast_remain_distinct() {
        let mut obs = observation(40);
        obs.tick = crate::stats::FOUNDRY_DRIP_START_TICK;
        obs.my_buildings = vec![
            building(1, BuildingKind::Foundry, TilePos::new(2, 2), true),
            building(2, BuildingKind::Reclaimer, TilePos::new(8, 2), true),
            building(3, BuildingKind::Extractor, TilePos::new(10, 2), true),
            building(4, BuildingKind::Reclaimer, TilePos::new(12, 2), false),
        ];
        obs.my_queues = vec![Vec::new(); 4];

        let resources = ResourceSnapshot::from_observation(&obs);
        let future = resources
            .forecast()
            .income_through(obs.tick.saturating_add(300));
        assert_eq!(resources.current_scrap().amount(), 40);
        assert!(
            future.amount() > 40,
            "completed income should be meaningful"
        );
        assert_eq!(resources.forecast().income_streams().len(), 3);

        let mut ledger = CommitmentLedger::new(&resources);
        assert_eq!(
            ledger.claim_scrap(
                owner(CommitmentDomain::Economy, 1),
                ScrapClaim::SpendNow(41),
            ),
            Err(ClaimConflict::InsufficientCurrentScrap {
                needed: 41,
                available: 40,
            }),
            "forecast income cannot become current credit"
        );
        assert_eq!(ledger.available_scrap(), 40);
    }

    #[test]
    fn income_forecast_uses_completed_sources_and_exact_cadences() {
        let mut obs = observation(0);
        obs.tick = 2_405;
        let mut refinery = building(9, BuildingKind::Reclaimer, TilePos::new(40, 3), true);
        refinery.tier = 1;
        obs.my_buildings = vec![
            building(7, BuildingKind::Extractor, TilePos::new(30, 3), true),
            building(2, BuildingKind::Foundry, TilePos::new(2, 3), true),
            building(5, BuildingKind::Extractor, TilePos::new(7, 3), true),
            refinery,
        ];
        obs.my_queues = vec![Vec::new(); 4];

        let forecast = ResourceSnapshot::from_observation(&obs).forecast;
        assert_eq!(forecast.observed_at(), 2_405);
        assert_eq!(
            forecast
                .income_streams()
                .iter()
                .map(|stream| (stream.source, stream.kind, stream.first_payment_tick))
                .collect::<Vec<_>>(),
            vec![
                (BuildingId(2), RecurringIncomeKind::Foundry, 2_459),
                (
                    BuildingId(5),
                    RecurringIncomeKind::SupportedExtractor,
                    2_419,
                ),
                (BuildingId(7), RecurringIncomeKind::RemoteExtractor, 2_409,),
                (BuildingId(9), RecurringIncomeKind::Refinery, 2_410),
            ]
        );
        assert_eq!(forecast.income_through(2_408).amount(), 0);
        assert_eq!(forecast.income_through(2_409).amount(), 1);
        assert_eq!(forecast.income_through(2_419).amount(), 6);
    }

    #[test]
    fn income_forecast_matches_authoritative_support_and_payment_boundaries() {
        let cases = [
            (
                "exact support radius",
                0,
                TilePos::new(11, 5),
                true,
                ExtractorIncome::Supported,
            ),
            (
                "one tile outside support",
                0,
                TilePos::new(10, 5),
                true,
                ExtractorIncome::Remote,
            ),
            (
                "wrong-owner foundry",
                1,
                TilePos::new(11, 5),
                true,
                ExtractorIncome::Remote,
            ),
            (
                "unfinished own foundry",
                0,
                TilePos::new(11, 5),
                false,
                ExtractorIncome::Remote,
            ),
        ];

        for (case, owner, anchor, built, expected_income) in cases {
            let mut state = income_parity_state(owner, anchor, built);
            let extractor = state
                .buildings()
                .iter()
                .find(|building| building.player == ME && building.kind == BuildingKind::Extractor)
                .expect("the authored Extractor stands")
                .id;
            assert_eq!(
                state.extractor_income(extractor),
                Some(expected_income),
                "{case}"
            );

            let resources =
                ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
            let stream = resources
                .forecast()
                .income_streams()
                .iter()
                .find(|stream| stream.source == extractor)
                .expect("the completed Extractor contributes a forecast stream");
            let expected_kind = if expected_income == ExtractorIncome::Supported {
                RecurringIncomeKind::SupportedExtractor
            } else {
                RecurringIncomeKind::RemoteExtractor
            };
            assert_eq!(stream.kind, expected_kind, "{case}");

            let starting_scrap = state.player(ME).scrap;
            let observed_at = state.current_tick();
            for deadline in [observed_at, observed_at + 1, observed_at + 2] {
                while state.current_tick() <= deadline {
                    state.tick(&[]);
                }
                assert_eq!(
                    state.player(ME).scrap - starting_scrap,
                    resources.forecast().income_through(deadline).amount(),
                    "{case} through tick {deadline}"
                );
            }
        }
    }

    #[test]
    fn additive_holds_are_independent_current_bank_commitments() {
        let resources = ResourceSnapshot::from_observation(&observation(300));
        let mut ledger = CommitmentLedger::new(&resources);
        let economy = owner(CommitmentDomain::Economy, 2);
        let strategic = owner(CommitmentDomain::Strategic, 4);

        ledger.claim_scrap(economy, ScrapClaim::Hold(40)).unwrap();
        ledger.claim_scrap(strategic, ScrapClaim::Hold(30)).unwrap();

        assert_eq!(ledger.held_scrap(), 70, "holds add");
        assert_eq!(ledger.available_scrap(), 230);
        assert_eq!(
            ledger
                .holds()
                .iter()
                .map(|row| row.owner)
                .collect::<Vec<_>>(),
            vec![economy, strategic]
        );
    }

    #[test]
    fn scrap_accounting_saturates_safely_and_failed_claims_are_inert() {
        let resources = ResourceSnapshot::from_observation(&observation(u32::MAX));
        let mut ledger = CommitmentLedger::new(&resources);
        let first = owner(CommitmentDomain::Legacy, 0);
        let second = owner(CommitmentDomain::Strategic, u32::MAX);
        ledger
            .claim_scrap(first, ScrapClaim::Hold(u32::MAX - 3))
            .unwrap();
        let before = ledger.clone();

        assert_eq!(
            ledger.claim_scrap(second, ScrapClaim::SpendNow(4)),
            Err(ClaimConflict::InsufficientCurrentScrap {
                needed: 4,
                available: 3,
            })
        );
        assert_eq!(ledger, before, "a rejected overflow-edge claim is inert");
        ledger.claim_scrap(second, ScrapClaim::SpendNow(3)).unwrap();
        assert_eq!(ledger.committed_scrap(), u32::MAX);
        assert_eq!(ledger.available_scrap(), 0);
    }

    #[test]
    fn resources_and_claims_use_canonical_id_and_row_major_order() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(200));
        assert_eq!(
            resources
                .units()
                .iter()
                .map(|unit| unit.id)
                .collect::<Vec<_>>(),
            vec![UnitId(1), UnitId(2), UnitId(3)]
        );
        assert_eq!(
            resources
                .builders()
                .iter()
                .map(|unit| unit.id)
                .collect::<Vec<_>>(),
            vec![UnitId(1), UnitId(2)]
        );
        assert_eq!(
            resources
                .producers()
                .iter()
                .map(|lane| lane.producer)
                .collect::<Vec<_>>(),
            vec![BuildingId(4), BuildingId(8)]
        );

        let mut ledger = CommitmentLedger::new(&resources);
        let a = owner(CommitmentDomain::Strategic, 9);
        let b = owner(CommitmentDomain::Economy, 2);
        ledger
            .claim_site(a, SiteFootprint::new(TilePos::new(1, 8), (1, 1)).unwrap())
            .unwrap();
        ledger
            .claim_site(b, SiteFootprint::new(TilePos::new(9, 2), (1, 1)).unwrap())
            .unwrap();
        ledger
            .claim_unit(a, UnitId(3), UnitClaimRole::Strategic)
            .unwrap();
        ledger
            .claim_unit(b, UnitId(1), UnitClaimRole::Strategic)
            .unwrap();
        ledger
            .claim_unit(a, UnitId(2), UnitClaimRole::Strategic)
            .unwrap();

        assert_eq!(
            ledger
                .unit_claims()
                .iter()
                .map(|claim| claim.unit)
                .collect::<Vec<_>>(),
            vec![UnitId(1), UnitId(2), UnitId(3)]
        );
        assert_eq!(
            ledger
                .site_claims()
                .iter()
                .map(|claim| claim.site.anchor)
                .collect::<Vec<_>>(),
            vec![TilePos::new(9, 2), TilePos::new(1, 8)],
            "site order must be (y, x), not TilePos's derived (x, y)"
        );
    }

    #[test]
    fn duplicate_and_cross_role_unit_claims_conflict_without_mutation() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(0));
        let mut ledger = CommitmentLedger::new(&resources);
        let strategic = owner(CommitmentDomain::Strategic, 1);
        let economy = owner(CommitmentDomain::Economy, 1);
        ledger
            .claim_unit(strategic, UnitId(3), UnitClaimRole::Strategic)
            .unwrap();
        let exact = ledger.clone();

        assert!(matches!(
            ledger.claim_unit(strategic, UnitId(3), UnitClaimRole::Strategic),
            Err(ClaimConflict::Unit {
                unit: UnitId(3),
                ..
            })
        ));
        assert_eq!(ledger, exact);

        ledger.claim_builder(economy, UnitId(1)).unwrap();
        let builder_claim = ledger.clone();
        assert!(matches!(
            ledger.claim_unit(strategic, UnitId(1), UnitClaimRole::Strategic),
            Err(ClaimConflict::Unit {
                unit: UnitId(1),
                ..
            })
        ));
        assert_eq!(ledger, builder_claim);
        assert_eq!(
            ledger.claim_unit(strategic, UnitId(99), UnitClaimRole::Strategic),
            Err(ClaimConflict::UnknownUnit(UnitId(99)))
        );
    }

    #[test]
    fn observed_builder_obligations_are_not_available_to_new_claims() {
        let mut obs = resource_observation(0);
        obs.my_units[1].site = Some(BuildingId(41));
        let resources = ResourceSnapshot::from_observation(&obs);
        let obligated = resources
            .builders()
            .iter()
            .find(|builder| builder.id == UnitId(1))
            .unwrap();
        assert_eq!(
            obligated.obligation,
            Some(BuilderObligation::Build(BuildingId(41)))
        );

        let mut ledger = CommitmentLedger::new(&resources);
        assert_eq!(
            ledger.claim_builder(owner(CommitmentDomain::Economy, 0), UnitId(1)),
            Err(ClaimConflict::BuilderUnavailable {
                unit: UnitId(1),
                obligation: Some(BuilderObligation::Build(BuildingId(41))),
            })
        );
        assert_eq!(
            ledger.claim_unit(
                owner(CommitmentDomain::Strategic, 0),
                UnitId(1),
                UnitClaimRole::Strategic,
            ),
            Err(ClaimConflict::BuilderUnavailable {
                unit: UnitId(1),
                obligation: Some(BuilderObligation::Build(BuildingId(41))),
            }),
            "another role cannot silently preempt observed non-preemptible work"
        );
        assert_eq!(
            ledger.claim_builder(owner(CommitmentDomain::Economy, 0), UnitId(3)),
            Err(ClaimConflict::BuilderUnavailable {
                unit: UnitId(3),
                obligation: None
            })
        );

        let mut queued_obs = resource_observation(0);
        queued_obs.my_queued_units = vec![UnitId(2)];
        let queued_resources = ResourceSnapshot::from_observation(&queued_obs);
        let queued = queued_resources
            .builders()
            .iter()
            .find(|builder| builder.id == UnitId(2))
            .expect("the queued Excavator remains an observed builder");
        assert_eq!(queued.obligation, Some(BuilderObligation::Queued));
        let mut queued_ledger = CommitmentLedger::new(&queued_resources);
        assert_eq!(
            queued_ledger.claim_builder(owner(CommitmentDomain::Economy, 1), UnitId(2)),
            Err(ClaimConflict::BuilderUnavailable {
                unit: UnitId(2),
                obligation: Some(BuilderObligation::Queued),
            })
        );
    }

    #[test]
    fn observed_builder_work_requires_an_exact_owned_import() {
        let mut obs = resource_observation(0);
        let promised = BuilderObligation::Found {
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(24, 9),
        };
        obs.my_units[1].founding = Some((BuildingKind::Foundry, TilePos::new(24, 9)));
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut ledger = CommitmentLedger::new(&resources);
        let expansion = owner(CommitmentDomain::Economy, 7);

        assert_eq!(
            ledger.import_builder_obligation(
                expansion,
                UnitId(1),
                BuilderObligation::Found {
                    kind: BuildingKind::Extractor,
                    anchor: TilePos::new(24, 9),
                },
            ),
            Err(ClaimConflict::BuilderObligationMismatch {
                unit: UnitId(1),
                expected: BuilderObligation::Found {
                    kind: BuildingKind::Extractor,
                    anchor: TilePos::new(24, 9),
                },
                observed: Some(promised),
            })
        );
        assert!(ledger.unit_claims().is_empty());

        ledger
            .import_builder_obligation(expansion, UnitId(1), promised)
            .unwrap();
        assert_eq!(
            ledger.unit_claims(),
            &[OwnedUnitClaim {
                unit: UnitId(1),
                owner: expansion,
                use_as: ExactUnitUse::Builder {
                    obligation: Some(promised),
                },
            }]
        );
        assert!(matches!(
            ledger.import_builder_obligation(expansion, UnitId(1), promised),
            Err(ClaimConflict::Unit {
                unit: UnitId(1),
                ..
            })
        ));
    }

    #[test]
    fn site_claims_conflict_and_production_appends_are_contiguous() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(0));
        let mut ledger = CommitmentLedger::new(&resources);
        let first = owner(CommitmentDomain::Economy, 3);
        let second = owner(CommitmentDomain::Strategic, 4);
        let site = SiteFootprint::new(TilePos::new(10, 10), (2, 2)).unwrap();
        ledger.claim_site(first, site).unwrap();
        let before_site_conflict = ledger.clone();
        assert!(matches!(
            ledger.claim_site(
                second,
                SiteFootprint::new(TilePos::new(11, 11), (2, 1)).unwrap()
            ),
            Err(ClaimConflict::Site { .. })
        ));
        assert_eq!(ledger, before_site_conflict);
        ledger
            .claim_site(
                second,
                SiteFootprint::new(TilePos::new(12, 10), (1, 2)).unwrap(),
            )
            .unwrap();

        let first_append = ledger
            .append_production(first, BuildingId(4), UnitKind::Lancer)
            .unwrap();
        let second_append = ledger
            .append_production(second, BuildingId(4), UnitKind::Bombard)
            .unwrap();
        assert_eq!(first_append.slot.queue_index, 0);
        assert_eq!(second_append.slot.queue_index, 1);
        assert_eq!(first_append.kind, UnitKind::Lancer);
        assert_eq!(second_append.kind, UnitKind::Bombard);
        assert_eq!(
            ledger.append_production(second, BuildingId(99), UnitKind::Lancer),
            Err(ClaimConflict::ProducerUnavailable(ProducerSlot {
                producer: BuildingId(99),
                queue_index: 0
            }))
        );
    }

    #[test]
    fn checkpoint_rolls_back_every_claim_class() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(500));
        let mut ledger = CommitmentLedger::new(&resources);
        let baseline_owner = owner(CommitmentDomain::Legacy, 0);
        ledger
            .claim_scrap(baseline_owner, ScrapClaim::Hold(20))
            .unwrap();
        let checkpoint = ledger.checkpoint();
        let speculative = owner(CommitmentDomain::Strategic, 8);

        ledger
            .claim_scrap(speculative, ScrapClaim::SpendNow(30))
            .unwrap();
        ledger
            .claim_scrap(speculative, ScrapClaim::Hold(40))
            .unwrap();
        ledger
            .claim_unit(speculative, UnitId(3), UnitClaimRole::Strategic)
            .unwrap();
        ledger.claim_builder(speculative, UnitId(1)).unwrap();
        ledger
            .claim_site(
                speculative,
                SiteFootprint::new(TilePos::new(20, 12), (2, 3)).unwrap(),
            )
            .unwrap();
        ledger
            .append_production(speculative, BuildingId(4), UnitKind::Lancer)
            .unwrap();
        assert_ne!(ledger, CommitmentLedger::new(&resources));

        ledger.rollback(checkpoint).unwrap();
        assert_eq!(ledger.held_scrap(), 20);
        assert_eq!(ledger.spent_scrap(), 0);
        assert!(ledger.unit_claims().is_empty());
        assert!(ledger.site_claims().is_empty());
        assert!(ledger.producer_claims().is_empty());
        assert_eq!(ledger.available_scrap(), 480);
    }

    #[test]
    fn checkpoint_refuses_a_different_observation_basis() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(100));
        let checkpoint = CommitmentLedger::new(&resources).checkpoint();
        let other = ResourceSnapshot::from_observation(&resource_observation(101));
        let mut ledger = CommitmentLedger::new(&other);
        let before = ledger.clone();
        assert_eq!(ledger.rollback(checkpoint), Err(CheckpointMismatch));
        assert_eq!(ledger, before);
    }

    #[test]
    fn releasing_an_owner_returns_holds_and_exact_claims_but_not_spending() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(300));
        let mut ledger = CommitmentLedger::new(&resources);
        let expansion = owner(CommitmentDomain::Economy, 11);
        ledger
            .claim_scrap(expansion, ScrapClaim::SpendNow(50))
            .unwrap();
        ledger.claim_scrap(expansion, ScrapClaim::Hold(40)).unwrap();
        ledger.claim_builder(expansion, UnitId(1)).unwrap();
        ledger
            .claim_site(
                expansion,
                SiteFootprint::new(TilePos::new(15, 4), (2, 2)).unwrap(),
            )
            .unwrap();
        ledger
            .append_production(expansion, BuildingId(4), UnitKind::Lancer)
            .unwrap();
        assert_eq!(ledger.available_scrap(), 210);

        assert_eq!(
            ledger.release(expansion),
            ReleaseSummary {
                held_scrap: 40,
                units: 1,
                sites: 1,
                producers: 1
            }
        );
        assert_eq!(
            ledger.spent_scrap(),
            50,
            "spent scrap is not refundable by planning release"
        );
        assert!(
            ledger
                .spending()
                .iter()
                .any(|row| row.owner == expansion && row.amount == 50)
        );
        assert_eq!(ledger.available_scrap(), 250);
        assert!(ledger.holds().is_empty());
    }

    #[test]
    fn releasing_an_append_reindexes_surviving_cross_owner_queue_claims() {
        let resources = ResourceSnapshot::from_observation(&resource_observation(0));
        let mut ledger = CommitmentLedger::new(&resources);
        let first = owner(CommitmentDomain::Economy, 1);
        let second = owner(CommitmentDomain::Strategic, 2);
        let third = owner(CommitmentDomain::Legacy, 3);

        ledger
            .append_production(first, BuildingId(4), UnitKind::Lancer)
            .unwrap();
        ledger
            .append_production(second, BuildingId(4), UnitKind::Bombard)
            .unwrap();
        ledger
            .append_production(first, BuildingId(8), UnitKind::Harvester)
            .unwrap();
        ledger
            .append_production(third, BuildingId(4), UnitKind::Lancer)
            .unwrap();

        let released = ledger.release(first);
        assert_eq!(released.producers, 2);
        assert_eq!(
            ledger
                .producer_claims()
                .iter()
                .map(|claim| (claim.owner, claim.slot, claim.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    second,
                    ProducerSlot {
                        producer: BuildingId(4),
                        queue_index: 0,
                    },
                    UnitKind::Bombard,
                ),
                (
                    third,
                    ProducerSlot {
                        producer: BuildingId(4),
                        queue_index: 1,
                    },
                    UnitKind::Lancer,
                ),
            ],
            "surviving plans keep their owners and relative order while holes close"
        );
        let appended = ledger
            .append_production(first, BuildingId(4), UnitKind::Lancer)
            .expect("a later append follows the reindexed contiguous prefix");
        assert_eq!(appended.slot.queue_index, 2);
    }

    #[test]
    fn release_recomputes_queue_timing_and_rollback_restores_the_original_claims() {
        let mut obs = resource_observation(0);
        obs.tick = 700;
        obs.my_queues[1] = vec![UnitKind::Lancer];
        let resources = ResourceSnapshot::from_observation(&obs);
        let lane = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(4))
            .unwrap();
        assert_eq!(lane.queued(), &[UnitKind::Lancer]);

        let mut ledger = CommitmentLedger::new(&resources);
        let a = owner(CommitmentDomain::Economy, 1);
        let b = owner(CommitmentDomain::Strategic, 2);
        let c = owner(CommitmentDomain::Legacy, 3);
        let d = owner(CommitmentDomain::Economy, 4);
        ledger
            .append_production(a, BuildingId(4), UnitKind::Lancer)
            .unwrap();
        ledger
            .append_production(b, BuildingId(4), UnitKind::Bombard)
            .unwrap();
        let c_before = ledger
            .append_production(c, BuildingId(4), UnitKind::Tender)
            .unwrap();
        let original = ledger.producer_claims().to_vec();
        let checkpoint = ledger.checkpoint();

        assert_eq!(ledger.release(b).producers, 1);
        let c_after = *ledger
            .producer_claims()
            .iter()
            .find(|claim| claim.owner == c)
            .unwrap();
        assert_eq!(c_before.slot.queue_index, 3);
        assert_eq!(c_after.slot.queue_index, 2);
        assert_eq!(
            c_before.timing.earliest_ready_tick - c_after.timing.earliest_ready_tick,
            Tick::from(UnitKind::Bombard.stats().train_ticks)
        );
        assert_eq!(
            c_before.timing.no_block_latest_ready_tick - c_after.timing.no_block_latest_ready_tick,
            Tick::from(UnitKind::Bombard.stats().train_ticks)
        );

        let appended = ledger
            .append_production(d, BuildingId(4), UnitKind::Sapper)
            .expect("the next append follows the reindexed contiguous prefix");
        assert_eq!(appended.slot.queue_index, 3);
        assert_eq!(
            appended.timing,
            lane.production_timing(&[UnitKind::Lancer, UnitKind::Tender, UnitKind::Sapper])
                .unwrap()
        );

        ledger.rollback(checkpoint).unwrap();
        assert_eq!(ledger.producer_claims(), original);
    }

    #[test]
    fn site_constructor_and_duplicate_claims_fail_without_malformed_state() {
        assert!(SiteFootprint::new(TilePos::new(4, 7), (0, 1)).is_none());
        assert!(SiteFootprint::new(TilePos::new(4, 7), (1, 0)).is_none());
        assert!(SiteFootprint::new(TilePos::new(4, 7), (-1, 2)).is_none());

        let resources = ResourceSnapshot::from_observation(&observation(0));
        let mut ledger = CommitmentLedger::new(&resources);
        let site = SiteFootprint::new(TilePos::new(i32::MAX, i32::MIN), (1, 1)).unwrap();
        ledger
            .claim_site(owner(CommitmentDomain::Economy, 0), site)
            .unwrap();
        let before = ledger.clone();
        assert!(matches!(
            ledger.claim_site(owner(CommitmentDomain::Strategic, 0), site),
            Err(ClaimConflict::Site { .. })
        ));
        assert_eq!(ledger, before);
    }

    #[test]
    fn ground_egress_matches_static_tile_blockers_not_unit_collisions_or_charges() {
        let mut obs = observation(0);
        let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
        let door = TilePos::new(9, 10);
        obs.known_rock =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .filter(|tile| *tile != door)
                .collect();
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        let mut occupant = unit(9, UnitKind::Sentinel);
        occupant.tile = door;
        obs.my_units.push(occupant);
        obs.enemy_buildings
            .push(building(6, BuildingKind::ScuttleCharge, door, true));
        obs.my_buildings.push(producer);
        obs.my_queues.push(Vec::new());

        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Open);

        obs.known_scrap.push((door, 1));
        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Blocked);
    }

    #[test]
    fn formerly_open_ground_egress_becomes_unknown_out_of_sight() {
        let mut obs = observation(0);
        let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
        let door = TilePos::new(9, 10);
        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .collect();
        obs.known_rock = ring.iter().copied().filter(|tile| *tile != door).collect();
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        obs.my_buildings.push(producer);
        obs.my_queues.push(Vec::new());

        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Open);

        for tile in ring {
            let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
            obs.visible[index] = false;
        }
        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Unknown);

        obs.known_rock.push(door);
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Blocked);
    }

    #[test]
    fn stale_remembered_blocker_does_not_prove_ground_egress_blocked() {
        let mut obs = observation(0);
        let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
        let door = TilePos::new(9, 10);
        let ring: Vec<_> =
            crate::tick::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
                .collect();
        obs.known_rock = ring.iter().copied().filter(|tile| *tile != door).collect();
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        let mut blocker = building(6, BuildingKind::Turret, door, true);
        blocker.seen = false;
        obs.enemy_buildings.push(blocker);
        obs.my_buildings.push(producer);
        obs.my_queues.push(Vec::new());
        for tile in ring {
            let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
            obs.visible[index] = false;
        }

        let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
        assert_eq!(lane.ground_egress, ProducerEgress::Unknown);
    }

    #[test]
    fn visible_building_blocker_stalls_authoritative_spawn_until_egress_opens() {
        let (mut state, producer, blocker) = blocked_egress_state();
        let resources = ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
        let lane = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == producer)
            .unwrap();
        assert_eq!(
            lane.production_timing(&[UnitKind::Lancer])
                .unwrap()
                .current_egress,
            ProducerEgress::Blocked
        );

        let blocked_report = state.tick(&[]);
        assert!(
            blocked_report.events.iter().all(|event| !matches!(
                event,
                Event::UnitTrained { building, .. } if *building == producer
            )),
            "a ready ground unit cannot cross a currently occupied doorstep"
        );
        assert_eq!(state.building(producer).unwrap().queue.len(), 1);

        state.buildings.retain(|building| building.id != blocker);
        state.rebuild_building_occupancy();
        let resources = ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
        let lane = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == producer)
            .unwrap();
        assert_eq!(
            lane.production_timing(&[UnitKind::Lancer])
                .unwrap()
                .current_egress,
            ProducerEgress::Open
        );

        let open_report = state.tick(&[]);
        assert!(open_report.events.iter().any(|event| matches!(
            event,
            Event::UnitTrained { building, kind, .. }
                if *building == producer && *kind == UnitKind::Lancer
        )));
    }

    #[test]
    fn empty_queue_starts_this_tick_and_unrepresentable_timing_is_rejected() {
        let mut obs = observation(0);
        obs.tick = 1_000;
        obs.my_buildings = vec![building(
            12,
            BuildingKind::Airworks,
            TilePos::new(20, 5),
            true,
        )];
        obs.my_queues = vec![Vec::new()];
        let resources = ResourceSnapshot::from_observation(&obs);
        let timing = resources.producers[0]
            .production_timing(&[UnitKind::Kestrel])
            .expect("an empty Airworks can start a legal command this tick");
        let ready = obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks) - 1;
        assert_eq!(timing.earliest_ready_tick, ready);
        assert_eq!(timing.no_block_latest_ready_tick, ready);
        assert_eq!(timing.current_egress, ProducerEgress::NotRequired);

        obs.tick = Tick::MAX;
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut ledger = CommitmentLedger::new(&resources);
        assert_eq!(
            ledger.append_production(
                owner(CommitmentDomain::Economy, 0),
                BuildingId(12),
                UnitKind::Kestrel,
            ),
            Err(ClaimConflict::ProducerTimingUnrepresentable {
                producer: BuildingId(12),
            })
        );
    }

    #[test]
    fn producer_timing_matches_authoritative_owner_visible_front_progress() {
        let mut base = crate::Scenario::skirmish().build().unwrap();
        base.player_mut(ME).scrap = u32::MAX;
        let producer = base
            .buildings
            .iter()
            .find(|building| building.player == ME && building.kind == BuildingKind::Foundry)
            .expect("skirmish has a player-zero Foundry")
            .id;
        {
            let foundry = base.building_mut(producer).unwrap();
            foundry.queue.clear();
            foundry.progress = 0;
        }

        let empty_obs = Observation::omniscient(&base, ME);
        let empty_timing = ResourceSnapshot::from_observation(&empty_obs)
            .producers
            .iter()
            .find(|lane| lane.producer == producer)
            .unwrap()
            .production_timing(&[UnitKind::Scuttler])
            .unwrap();
        let empty_actual = run_until_trained(&mut base, producer, UnitKind::Scuttler);
        assert_eq!(empty_timing.earliest_ready_tick, empty_actual);
        assert_eq!(empty_timing.no_block_latest_ready_tick, empty_actual);

        let mut almost_done = crate::Scenario::skirmish().build().unwrap();
        almost_done.player_mut(ME).scrap = u32::MAX;
        let producer = almost_done
            .buildings
            .iter()
            .find(|building| building.player == ME && building.kind == BuildingKind::Foundry)
            .unwrap()
            .id;
        {
            let foundry = almost_done.building_mut(producer).unwrap();
            foundry.queue.clear();
            foundry.queue.push_back(UnitKind::Harvester);
            foundry.progress = UnitKind::Harvester.stats().train_ticks - 1;
        }
        let mut just_started = almost_done.clone();
        just_started.building_mut(producer).unwrap().progress = 0;
        let almost_done_obs = Observation::omniscient(&almost_done, ME);
        let just_started_obs = Observation::omniscient(&just_started, ME);
        assert_ne!(almost_done_obs, just_started_obs);
        let almost_done_timing = ResourceSnapshot::from_observation(&almost_done_obs)
            .producers
            .iter()
            .find(|lane| lane.producer == producer)
            .unwrap()
            .production_timing(&[UnitKind::Sentinel])
            .unwrap();
        let just_started_timing = ResourceSnapshot::from_observation(&just_started_obs)
            .producers
            .iter()
            .find(|lane| lane.producer == producer)
            .unwrap()
            .production_timing(&[UnitKind::Sentinel])
            .unwrap();

        let almost_done_actual = run_until_trained(&mut almost_done, producer, UnitKind::Sentinel);
        let just_started_actual =
            run_until_trained(&mut just_started, producer, UnitKind::Sentinel);
        assert_eq!(almost_done_timing.earliest_ready_tick, almost_done_actual);
        assert_eq!(
            almost_done_timing.no_block_latest_ready_tick,
            almost_done_actual
        );
        assert_eq!(just_started_timing.earliest_ready_tick, just_started_actual);
        assert_eq!(
            just_started_timing.no_block_latest_ready_tick,
            just_started_actual
        );
    }

    #[test]
    fn misaligned_queue_progress_falls_back_to_conservative_timing() {
        let mut obs = observation(0);
        obs.tick = 100;
        obs.my_buildings = vec![building(
            12,
            BuildingKind::Airworks,
            TilePos::new(20, 5),
            true,
        )];
        obs.my_queues = vec![vec![UnitKind::Buzzard]];
        obs.my_queue_progress = vec![12, u32::MAX];

        assert_eq!(obs.own_queue_progress(0), None);
        let resources = ResourceSnapshot::from_observation(&obs);
        let timing = resources.producers[0]
            .production_timing(&[UnitKind::Kestrel])
            .expect("the valid queue still defines conservative timing");
        assert_eq!(
            timing.earliest_ready_tick,
            obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks),
            "the front item may be one production tick from completion"
        );
        assert_eq!(
            timing.no_block_latest_ready_tick,
            obs.tick
                + Tick::from(UnitKind::Buzzard.stats().train_ticks)
                + Tick::from(UnitKind::Kestrel.stats().train_ticks)
                - 1,
            "misaligned progress cannot create an optimistic deadline promise"
        );
    }

    #[test]
    fn impossible_queue_progress_falls_back_without_crediting_empty_queues() {
        let mut obs = observation(0);
        obs.tick = 100;
        obs.my_buildings = vec![building(
            12,
            BuildingKind::Airworks,
            TilePos::new(20, 5),
            true,
        )];
        obs.my_queues = vec![vec![UnitKind::Buzzard]];
        obs.my_queue_progress = vec![UnitKind::Buzzard.stats().train_ticks + 1];

        assert_eq!(obs.own_queue_progress(0), None);
        let resources = ResourceSnapshot::from_observation(&obs);
        let timing = resources.producers[0]
            .production_timing(&[UnitKind::Kestrel])
            .expect("the valid queue still defines conservative timing");
        assert_eq!(
            timing.earliest_ready_tick,
            obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks)
        );
        assert_eq!(
            timing.no_block_latest_ready_tick,
            obs.tick
                + Tick::from(UnitKind::Buzzard.stats().train_ticks)
                + Tick::from(UnitKind::Kestrel.stats().train_ticks)
                - 1,
            "overlong progress cannot create optimistic deadline evidence"
        );

        obs.my_queues[0].clear();
        obs.my_queue_progress[0] = 1;
        assert_eq!(
            obs.own_queue_progress(0),
            None,
            "an empty queue cannot carry production progress"
        );
        obs.my_queue_progress[0] = 0;
        assert_eq!(obs.own_queue_progress(0), Some(0));
    }

    #[test]
    fn unrepresentable_income_cadence_is_not_saturated_to_tick_max() {
        assert_eq!(next_multiple_at_or_after(Tick::MAX, 24), None);
        let mut obs = observation(0);
        obs.tick = Tick::MAX;
        obs.my_buildings = vec![building(
            1,
            BuildingKind::Reclaimer,
            TilePos::new(2, 2),
            true,
        )];
        obs.my_queues = vec![Vec::new()];
        assert!(
            ResourceSnapshot::from_observation(&obs)
                .forecast()
                .income_streams()
                .is_empty()
        );
    }

    #[test]
    fn current_prerequisites_gate_new_appends_without_losing_prepaid_queue_work() {
        let mut obs = observation(0);
        obs.tick = 1_000;
        obs.my_buildings = vec![
            building(12, BuildingKind::Airworks, TilePos::new(20, 5), true),
            building(13, BuildingKind::Crucible, TilePos::new(24, 5), false),
        ];
        obs.my_queues = vec![vec![UnitKind::Condor], Vec::new()];

        let resources = ResourceSnapshot::from_observation(&obs);
        let airworks = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(12))
            .expect("the completed Airworks remains a production lane");
        assert_eq!(airworks.queued(), &[UnitKind::Condor]);
        assert!(!airworks.trainable().contains(&UnitKind::Condor));
        assert!(resources.producer_slots_for(UnitKind::Condor).is_empty());
        let mut ledger = CommitmentLedger::new(&resources);
        assert_eq!(
            ledger.append_production(
                owner(CommitmentDomain::Strategic, 0),
                BuildingId(12),
                UnitKind::Condor,
            ),
            Err(ClaimConflict::ProducerCannotTrain {
                producer: BuildingId(12),
                kind: UnitKind::Condor,
            })
        );

        obs.my_buildings[1].built = true;
        let resources = ResourceSnapshot::from_observation(&obs);
        let airworks = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(12))
            .unwrap();
        assert_eq!(airworks.queued(), &[UnitKind::Condor]);
        assert!(airworks.trainable().contains(&UnitKind::Condor));
        let mut ledger = CommitmentLedger::new(&resources);
        let appended = ledger
            .append_production(
                owner(CommitmentDomain::Strategic, 0),
                BuildingId(12),
                UnitKind::Condor,
            )
            .expect("the completed Crucible unlocks another Condor");
        assert_eq!(appended.slot.queue_index, 1);
        assert_eq!(
            appended.timing,
            airworks
                .production_timing(&[UnitKind::Condor])
                .expect("the existing prepaid Condor remains ahead of the new append")
        );
    }

    #[test]
    fn completed_producers_and_live_queues_define_deterministic_capacity() {
        let mut obs = observation(0);
        obs.tick = 1_000;
        let mut cupric_only = building(12, BuildingKind::Airworks, TilePos::new(20, 5), true);
        cupric_only.tier = 0;
        obs.my_buildings = vec![
            building(20, BuildingKind::Foundry, TilePos::new(2, 2), true),
            building(5, BuildingKind::Fabricator, TilePos::new(8, 2), false),
            cupric_only,
            building(8, BuildingKind::Fabricator, TilePos::new(8, 8), true),
        ];
        obs.my_queues = vec![
            vec![UnitKind::Harvester, UnitKind::Sentinel],
            Vec::new(),
            vec![UnitKind::Kestrel],
            vec![UnitKind::Lancer; QUEUE_CAP],
        ];

        let resources = ResourceSnapshot::from_observation(&obs);
        assert_eq!(
            resources
                .producers()
                .iter()
                .map(|lane| lane.producer)
                .collect::<Vec<_>>(),
            vec![BuildingId(8), BuildingId(12), BuildingId(20)]
        );
        assert_eq!(
            resources.producer_slots().len(),
            (QUEUE_CAP - 1) + (QUEUE_CAP - 2)
        );
        assert_eq!(
            resources.producer_slots()[0],
            ProducerSlot {
                producer: BuildingId(12),
                queue_index: 1
            }
        );
        assert!(
            resources
                .producer_slots()
                .iter()
                .all(|slot| slot.producer != BuildingId(5) && slot.producer != BuildingId(8))
        );
        assert!(
            resources.producer_slots_for(UnitKind::Moth).is_empty(),
            "a Ferrous seat cannot train Cupric air"
        );
        assert_eq!(
            resources.producer_slots_for(UnitKind::Kestrel).len(),
            QUEUE_CAP - 1
        );
        assert!(
            resources
                .producers()
                .iter()
                .find(|lane| lane.producer == BuildingId(12))
                .unwrap()
                .trainable()
                .contains(&UnitKind::Kestrel)
        );

        let foundry = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(20))
            .unwrap();
        assert_eq!(foundry.queued(), &[UnitKind::Harvester, UnitKind::Sentinel]);
        let timing = foundry
            .production_timing(&[UnitKind::Scuttler])
            .expect("the Foundry has one open legal queue position");
        assert_eq!(
            timing.earliest_ready_tick,
            obs.tick
                + Tick::from(UnitKind::Sentinel.stats().train_ticks)
                + Tick::from(UnitKind::Scuttler.stats().train_ticks),
            "the unknown front item may be one tick from completion"
        );
        assert_eq!(
            timing.no_block_latest_ready_tick,
            obs.tick
                + Tick::from(UnitKind::Harvester.stats().train_ticks)
                + Tick::from(UnitKind::Sentinel.stats().train_ticks)
                + Tick::from(UnitKind::Scuttler.stats().train_ticks)
                - 1,
            "with no egress delay the unknown front item may have zero progress"
        );
        assert_eq!(timing.current_egress, ProducerEgress::Open);
        assert_eq!(
            foundry.production_timing(&[UnitKind::Sentinel; QUEUE_CAP - 1]),
            None,
            "the plan cannot exceed real queue capacity"
        );
        assert_eq!(
            foundry.production_timing(&[UnitKind::Lancer]),
            None,
            "the plan must use this producer's legal roster"
        );
    }

    #[test]
    fn owner_rows_are_canonical_across_current_domains() {
        let resources = ResourceSnapshot::from_observation(&observation(3));
        let mut ledger = CommitmentLedger::new(&resources);
        let domains = [
            CommitmentDomain::Economy,
            CommitmentDomain::Strategic,
            CommitmentDomain::Legacy,
        ];
        for (sequence, domain) in domains.into_iter().enumerate().rev() {
            ledger
                .claim_scrap(
                    owner(domain, u32::try_from(sequence).unwrap()),
                    ScrapClaim::Hold(1),
                )
                .unwrap();
        }

        assert_eq!(
            ledger
                .holds()
                .iter()
                .map(|row| row.owner.domain)
                .collect::<Vec<_>>(),
            domains
        );
    }
}
