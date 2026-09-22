//! Bounded candidate refinement with a revalidated incumbent and a rotating tail.

use super::Progress;

const LIFETIME: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Alternatives<Key> {
    cursor: usize,
    incumbent: Option<(u64, Key)>,
    tick: Option<u64>,
    remaining: usize,
}

impl<Key> Default for Alternatives<Key> {
    fn default() -> Self {
        Self {
            cursor: 0,
            incumbent: None,
            tick: None,
            remaining: 0,
        }
    }
}

impl<Key: Copy + Eq> Alternatives<Key> {
    pub(super) fn retained(&self, tick: u64) -> Option<Key> {
        self.incumbent
            .filter(|(started, _)| tick.saturating_sub(*started) < LIFETIME)
            .map(|(_, key)| key)
    }

    pub(super) fn clear(&mut self) {
        self.incumbent = None;
    }

    pub(super) fn advance<T>(
        &mut self,
        tick: u64,
        candidates: &[Key],
        limit: usize,
        mut evaluate: impl FnMut(Key) -> Progress<T>,
        better: impl Fn(&T, &T) -> bool,
        mut try_claim: impl FnMut() -> bool,
    ) -> Progress<T> {
        if candidates.is_empty() {
            self.clear();
            return Progress::ProvenInfeasible;
        }
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.remaining = limit;
        }
        let retained = self.retained(tick).filter(|key| candidates.contains(key));
        let mut pending = None;
        if let Some(key) = retained {
            match evaluate(key) {
                Progress::Ready(value) => return Progress::Ready(value),
                Progress::Deferred => pending = self.incumbent,
                Progress::ProvenInfeasible => {}
            }
        }
        self.clear();
        let mut selected = None;
        for _ in 0..candidates.len().min(self.remaining) {
            if !try_claim() {
                break;
            }
            self.remaining -= 1;
            let key = candidates[self.cursor % candidates.len()];
            self.cursor = (self.cursor + 1) % candidates.len();
            if Some(key) == retained {
                continue;
            }
            match evaluate(key) {
                Progress::Ready(value)
                    if selected
                        .as_ref()
                        .is_none_or(|(_, prior)| better(&value, prior)) =>
                {
                    selected = Some((key, value));
                }
                Progress::Deferred if pending.is_none() => pending = Some((tick, key)),
                _ => {}
            }
        }
        if let Some((key, value)) = selected {
            self.incumbent = Some((tick, key));
            Progress::Ready(value)
        } else {
            self.incumbent = pending;
            Progress::Deferred
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_work_resumes_without_starving_other_candidates_or_refilling_the_pass() {
        let mut work = Alternatives::default();
        let mut seen = Vec::new();
        assert_eq!(
            work.advance(
                0,
                &[0, 1, 2, 3, 4],
                2,
                |key| {
                    seen.push(key);
                    if key == 0 {
                        Progress::Deferred
                    } else {
                        Progress::ProvenInfeasible
                    }
                },
                |_: &(), _| false,
                || true
            ),
            Progress::Deferred
        );
        assert_eq!(seen, [0, 1]);
        seen.clear();
        work.advance(
            0,
            &[0, 1, 2, 3, 4],
            2,
            |key| {
                seen.push(key);
                Progress::<()>::Deferred
            },
            |_, _| false,
            || true,
        );
        assert_eq!(
            seen,
            [0],
            "same-tick calls can revalidate, but cannot admit more alternatives"
        );
        seen.clear();
        work.advance(
            12,
            &[0, 1, 2, 3, 4],
            2,
            |key| {
                seen.push(key);
                Progress::<()>::Deferred
            },
            |_, _| false,
            || true,
        );
        assert_eq!(seen, [0, 2, 3]);
        assert_eq!(
            work.advance(
                24,
                &[0, 1, 2, 3, 4],
                2,
                |key| {
                    if key == 0 {
                        Progress::Ready(17)
                    } else {
                        panic!("finished incumbent should win")
                    }
                },
                |_, _| false,
                || true
            ),
            Progress::Ready(17)
        );
    }

    #[test]
    fn stale_or_removed_candidates_are_released_and_unexamined_work_is_never_infeasible() {
        let mut work = Alternatives::default();
        assert_eq!(
            work.advance(
                0,
                &[1, 2, 3],
                2,
                |_| Progress::<()>::Deferred,
                |_, _| false,
                || false
            ),
            Progress::Deferred
        );
        assert!(work.retained(0).is_none());
        work.advance(
            0,
            &[1, 2, 3],
            2,
            |_| Progress::<()>::Deferred,
            |_, _| false,
            || true,
        );
        assert_eq!(work.retained(119), Some(1));
        assert_eq!(work.retained(120), None);
        let mut seen = Vec::new();
        work.advance(
            12,
            &[2, 3],
            2,
            |key| {
                seen.push(key);
                Progress::<()>::Deferred
            },
            |_, _| false,
            || true,
        );
        assert!(!seen.contains(&1));
        assert_eq!(
            work.advance(
                24,
                &[],
                2,
                |_| Progress::<()>::Deferred,
                |_, _| false,
                || true
            ),
            Progress::ProvenInfeasible
        );
        assert!(work.retained(24).is_none());
    }
}
