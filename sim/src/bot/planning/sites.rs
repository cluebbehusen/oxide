//! Bounded site refinement with independent role cursors and live revalidation.

use super::{Progress, WorkBudget};
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use std::collections::BTreeMap;

const CANDIDATES_PER_DECISION: usize = 4;
const INCUMBENT_LIFETIME: u64 = 120;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct SiteWork {
    roles: BTreeMap<BuildingKind, RoleWork>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RoleWork {
    cursor: usize,
    incumbent: Option<(u64, TilePos)>,
    tick: Option<u64>,
    remaining: usize,
}

impl SiteWork {
    pub(in crate::bot) fn retained(&self, tick: u64, kind: BuildingKind) -> Option<TilePos> {
        self.roles
            .get(&kind)?
            .incumbent
            .filter(|(started_at, _)| tick.saturating_sub(*started_at) < INCUMBENT_LIFETIME)
            .map(|(_, anchor)| anchor)
    }

    pub(in crate::bot) fn clear_incumbent(&mut self, kind: BuildingKind) {
        if let Some(role) = self.roles.get_mut(&kind) {
            role.incumbent = None;
        }
    }

    pub(in crate::bot) fn advance_ranked<T>(
        &mut self,
        tick: u64,
        kind: BuildingKind,
        anchors: &[TilePos],
        mut evaluate: impl FnMut(TilePos) -> Option<T>,
        better: impl Fn(&T, &T) -> bool,
    ) -> Progress<T> {
        if anchors.is_empty() {
            return Progress::ProvenInfeasible;
        }
        let role = self.roles.entry(kind).or_default();
        if role.tick != Some(tick) {
            role.tick = Some(tick);
            role.remaining = CANDIDATES_PER_DECISION;
        }
        if let Some((started_at, anchor)) = role.incumbent
            && tick.saturating_sub(started_at) < INCUMBENT_LIFETIME
            && anchors.contains(&anchor)
            && let Some(candidate) = evaluate(anchor)
        {
            return Progress::Ready(candidate);
        }
        role.incumbent = None;
        let mut budget = WorkBudget::new(role.remaining);
        let mut selected: Option<(TilePos, T)> = None;
        for _ in 0..anchors.len().min(role.remaining) {
            if !budget.charge(1) {
                break;
            }
            let anchor = anchors[role.cursor % anchors.len()];
            role.cursor = (role.cursor + 1) % anchors.len();
            if let Some(candidate) = evaluate(anchor)
                && selected
                    .as_ref()
                    .is_none_or(|(_, prior)| better(&candidate, prior))
            {
                selected = Some((anchor, candidate));
            }
        }
        role.remaining -= budget.spent();
        match selected {
            Some((anchor, candidate)) => {
                role.incumbent = Some((tick, anchor));
                Progress::Ready(candidate)
            }
            None => Progress::Deferred,
        }
    }
}
