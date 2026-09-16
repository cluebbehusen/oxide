//! Bounded site refinement with independent role cursors and live revalidation.

use super::{Progress, alternatives::Alternatives};
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct SiteWork {
    roles: BTreeMap<BuildingKind, Alternatives<TilePos>>,
}

impl SiteWork {
    pub(in crate::bot) fn retained(&self, tick: u64, kind: BuildingKind) -> Option<TilePos> {
        self.roles.get(&kind)?.retained(tick)
    }

    pub(in crate::bot) fn clear_incumbent(&mut self, kind: BuildingKind) {
        if let Some(role) = self.roles.get_mut(&kind) {
            role.clear();
        }
    }

    pub(in crate::bot) fn advance_ranked<T>(
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
