//! Incremental voluntary site selection; exact validation remains query-local.

use super::*;
#[cfg(test)]
use crate::bot::planning::Progress;

#[cfg(test)]
use crate::bot::planning::sites::SiteWork;

#[cfg(test)]
impl SiteWork {
    pub(super) fn advance(
        &mut self,
        tick: u64,
        profile: DefenseProfile,
        anchors: &[TilePos],
        evaluate: impl FnMut(TilePos) -> Option<(Candidate, UnitId)>,
    ) -> Progress<(Candidate, UnitId)> {
        self.advance_ranked(
            tick,
            profile.kind,
            anchors,
            evaluate,
            |candidate, prior| candidate.0.key(profile) > prior.0.key(profile),
            || true,
        )
    }
}

pub(super) fn rank(
    anchors: &mut [TilePos],
    assets: &[DefendedAsset],
    approaches: &[Approach],
    profile: DefenseProfile,
) {
    let mut frontage = BTreeMap::<TilePos, u32>::new();
    for approach in approaches {
        let depth = usize::try_from(profile.candidate_reach).unwrap_or(0);
        let index = approach.path.len().saturating_sub(depth + 1);
        if let Some(&tile) = approach.path.get(index) {
            let value = assets[approach.asset].value;
            frontage
                .entry(tile)
                .and_modify(|prior| *prior = (*prior).max(value))
                .or_insert(value);
        }
    }
    anchors.sort_by_cached_key(|anchor| {
        let value = frontage
            .iter()
            .map(|(tile, value)| {
                u64::from(*value).saturating_mul(256) / (1 + anchor.manhattan(*tile) as u64)
            })
            .fold(0_u64, u64::saturating_add);
        (Reverse(value), anchor.y, anchor.x)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(anchor: TilePos) -> (Candidate, UnitId) {
        (
            Candidate {
                anchor,
                builder_travel: 0,
                coverage: Coverage {
                    new: 1,
                    ..Coverage::empty()
                },
                threat_distance: 1,
            },
            UnitId(1),
        )
    }

    #[test]
    fn exhausted_slices_resume_beyond_the_first_invalid_sites() {
        let anchors = (0..12).map(|x| TilePos::new(x, 2)).collect::<Vec<_>>();
        let profile = DefenseProfile::for_kind(BuildingKind::Turret).unwrap();
        let mut work = SiteWork::default();
        let mut visited = Vec::new();
        for tick in [24, 24, 48] {
            assert_eq!(
                work.advance(tick, profile, &anchors, |anchor| {
                    visited.push(anchor);
                    None
                }),
                Progress::Deferred
            );
        }
        assert_eq!(visited, anchors[..8]);
        let ready = work.advance(72, profile, &anchors, |anchor| {
            visited.push(anchor);
            (anchor == anchors[9]).then(|| candidate(anchor))
        });
        assert_eq!(ready, Progress::Ready(candidate(anchors[9])));
        assert_eq!(visited, anchors);
    }

    #[test]
    fn incumbents_are_revalidated_and_other_roles_keep_their_allowance() {
        let anchors = (0..12).map(|x| TilePos::new(x, 2)).collect::<Vec<_>>();
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).unwrap();
        let bastion = DefenseProfile::for_kind(BuildingKind::Bastion).unwrap();
        let mut work = SiteWork::default();
        let selected = work.advance(24, turret, &anchors, |anchor| Some(candidate(anchor)));
        assert!(matches!(selected, Progress::Ready(_)));
        assert_eq!(
            work.advance(24, turret, &anchors, |_| None),
            Progress::Deferred,
            "an incumbent whose current safety check fails cannot be returned"
        );
        assert!(matches!(
            work.advance(24, bastion, &anchors, |anchor| Some(candidate(anchor))),
            Progress::Ready(_)
        ));
        let mut clone = work.clone();
        assert_eq!(
            work.advance(48, turret, &anchors, |anchor| Some(candidate(anchor))),
            clone.advance(48, turret, &anchors, |anchor| Some(candidate(anchor)))
        );
    }

    #[test]
    fn expiring_an_uncommitted_incumbent_does_not_restart_the_failed_prefix() {
        let anchors = (0..12).map(|x| TilePos::new(x, 2)).collect::<Vec<_>>();
        let profile = DefenseProfile::for_kind(BuildingKind::Turret).unwrap();
        let mut work = SiteWork::default();
        assert!(matches!(
            work.advance(24, profile, &anchors, |anchor| Some(candidate(anchor))),
            Progress::Ready(_)
        ));
        let mut visited = Vec::new();
        assert_eq!(
            work.advance(144, profile, &anchors, |anchor| {
                visited.push(anchor);
                None
            }),
            Progress::Deferred
        );
        assert_eq!(visited, anchors[4..8]);
    }
}
