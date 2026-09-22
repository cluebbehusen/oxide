//! Bounded site refinement with independent role cursors and live revalidation.

use super::{Progress, alternatives::Alternatives};
use chassis::grid::TilePos;
use oxide_sim::stats::BuildingKind;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SiteWork {
    roles: BTreeMap<BuildingKind, Alternatives<TilePos>>,
}

impl SiteWork {
    pub(super) fn valid_checkpoint(&self, tick: u64) -> bool {
        self.roles
            .values()
            .all(|work| work.valid_checkpoint(tick, 4))
    }

    pub(crate) fn retained(&self, tick: u64, kind: BuildingKind) -> Option<TilePos> {
        self.roles.get(&kind)?.retained(tick)
    }

    pub(crate) fn clear_incumbent(&mut self, kind: BuildingKind) {
        if let Some(role) = self.roles.get_mut(&kind) {
            role.clear();
        }
    }

    pub(crate) fn advance_ranked<T>(
        &mut self,
        tick: u64,
        kind: BuildingKind,
        anchors: &[TilePos],
        evaluate: impl FnMut(TilePos) -> Progress<T>,
        better: impl Fn(&T, &T) -> bool,
        try_claim: impl FnMut() -> bool,
    ) -> Progress<T> {
        self.roles
            .entry(kind)
            .or_default()
            .advance(tick, anchors, 4, evaluate, better, try_claim)
    }
}
