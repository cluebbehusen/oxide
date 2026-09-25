use super::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

#[derive(Serialize, Deserialize)]
enum Preparation<O, T> {
    Preparing(O),
    Searching(T),
    Complete,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedJob<O, T> {
    query_purpose: QueryPurpose,
    used: u64,
    preparation: Preparation<O, T>,
}

impl Serialize for Job {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let preparation = if self.ready.is_some() {
            Preparation::Complete
        } else if let Some(traversal) = &self.traversal {
            Preparation::Searching(traversal)
        } else {
            Preparation::Preparing(&self.open)
        };
        SavedJob {
            query_purpose: self.query_purpose,
            used: self.used,
            preparation,
        }
        .serialize(serializer)
    }
}

type SavedJobs = BTreeMap<(Option<BlockedRect>, Vec<TilePos>), SavedJob<Vec<bool>, DistanceWork>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPreparation {
    generation: Option<Generation>,
    jobs: SavedJobs,
    next_pending: usize,
}

impl<'de> Deserialize<'de> for ApproachPreparation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let saved = SavedPreparation::deserialize(deserializer)?;
        let cells = match &saved.generation {
            Some(generation)
                if (1..=oxide_sim::map::MAX_MAP_EDGE as i32).contains(&generation.width)
                    && (1..=oxide_sim::map::MAX_MAP_EDGE as i32).contains(&generation.height) =>
            {
                let cells = generation.width as usize * generation.height as usize;
                if generation.blocked.len() != cells {
                    return Err(D::Error::custom("invalid approach generation"));
                }
                cells
            }
            None if saved.jobs.is_empty() => 0,
            _ => return Err(D::Error::custom("invalid approach generation")),
        };
        let mut pending = 0;
        let mut ready_bytes = 0usize;
        for ((_, goals), job) in &saved.jobs {
            match &job.preparation {
                Preparation::Complete => {
                    ready_bytes = ready_bytes
                        .checked_add(cells * size_of::<u32>())
                        .and_then(|n| n.checked_add(size_of_val(goals.as_slice())))
                        .and_then(|n| n.checked_add(512))
                        .ok_or_else(|| D::Error::custom("approach storage overflow"))?;
                }
                Preparation::Preparing(open) => {
                    pending += 1;
                    if open.len() > cells {
                        return Err(D::Error::custom("invalid approach preparation"));
                    }
                }
                Preparation::Searching(work) => {
                    pending += 1;
                    let generation = saved.generation.as_ref().unwrap();
                    if !work.valid_checkpoint(generation.width, generation.height) {
                        return Err(D::Error::custom("invalid approach search"));
                    }
                }
            }
            if pending > PENDING_FIELDS || ready_bytes > READY_BYTES {
                return Err(D::Error::custom("approach storage exceeds budget"));
            }
        }

        let mut jobs = BTreeMap::new();
        for ((overlay, goals), saved_job) in saved.jobs {
            let mut job = Job {
                query_purpose: saved_job.query_purpose,
                used: saved_job.used,
                open: Vec::new(),
                traversal: None,
                ready: None,
            };
            match saved_job.preparation {
                Preparation::Preparing(open) => job.open = open,
                Preparation::Searching(work) => job.traversal = Some(work),
                Preparation::Complete => {
                    // Rebuilding an already-completed answer must not spend the live
                    // planning allowance or change when another query becomes ready.
                    let result = job.advance(
                        job.query_purpose,
                        saved.generation.as_ref().unwrap(),
                        &goals,
                        overlay,
                        &mut WorkBudget::new(usize::MAX),
                    );
                    if !matches!(result, Progress::Ready(_)) {
                        return Err(D::Error::custom("approach reconstruction did not finish"));
                    }
                }
            }
            jobs.insert((overlay, goals), job);
        }
        Ok(Self {
            generation: saved.generation,
            jobs,
            next_pending: saved.next_pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_preserve_mixed_progress_and_completed_overlay_distances() {
        let blocked = vec![false; 64];
        let grid = KnownGrid::new(8, 8, &blocked).unwrap();
        let mut work = ApproachPreparation::default();
        for (goal, allowance) in [(1, usize::MAX), (3, 5), (5, 70)] {
            work.advance(
                QueryPurpose::NavigationTest,
                24,
                grid,
                &[TilePos::new(goal, 7)],
                Some(BlockedRect {
                    anchor: TilePos::new(2, 2),
                    size: (2, 2),
                }),
                &mut WorkBudget::new(allowance),
            );
        }
        assert_eq!(work.counts(), (2, 3));
        let mut restored = crate::checkpoint::round_trip(&work);
        for tick in 25..50 {
            work.resume_pending(tick, &mut WorkBudget::new(20));
            restored.resume_pending(tick, &mut WorkBudget::new(20));
            assert_eq!(work, restored);
        }
    }

    #[test]
    fn completed_field_storage_scales_with_recipes_not_distance_arrays() {
        let blocked = vec![false; 128 * 128];
        let grid = KnownGrid::new(128, 128, &blocked).unwrap();
        let mut work = ApproachPreparation::default();
        for x in 0..32 {
            assert!(matches!(
                work.advance(
                    QueryPurpose::NavigationTest,
                    24,
                    grid,
                    &[TilePos::new(x, 127)],
                    None,
                    &mut WorkBudget::new(usize::MAX),
                ),
                Progress::Ready(_)
            ));
        }
        assert_eq!(work.counts(), (0, 32));
        let mut bytes = Vec::new();
        ciborium::into_writer(&work, &mut bytes).unwrap();
        assert!(
            bytes.len() <= 32 * 1024,
            "completed recipes grew to {} bytes",
            bytes.len()
        );
        let mut restored: ApproachPreparation = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(work, restored);
        let mut budget = WorkBudget::new(0);
        for x in 0..32 {
            assert!(matches!(
                restored.advance(
                    QueryPurpose::NavigationTest,
                    25,
                    grid,
                    &[TilePos::new(x, 127)],
                    None,
                    &mut budget,
                ),
                Progress::Ready(_)
            ));
        }
        assert_eq!(
            budget.spent(),
            0,
            "restoration cannot turn ready answers back into budgeted work"
        );
    }

    #[test]
    fn oversized_complete_recipes_are_rejected_before_reconstruction() {
        let jobs = (0..130)
            .map(|n| {
                (
                    (None, vec![TilePos::new(n, 0)]),
                    SavedJob::<Vec<bool>, DistanceWork> {
                        query_purpose: QueryPurpose::NavigationTest,
                        used: 0,
                        preparation: Preparation::Complete,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        #[derive(Serialize)]
        struct Wire {
            generation: Generation,
            jobs: SavedJobs,
            next_pending: usize,
        }
        let wire = Wire {
            generation: Generation {
                width: 256,
                height: 256,
                blocked: vec![false; 256 * 256],
            },
            jobs,
            next_pending: 0,
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&wire, &mut bytes).unwrap();
        assert!(ciborium::from_reader::<ApproachPreparation, _>(bytes.as_slice()).is_err());
    }
}
