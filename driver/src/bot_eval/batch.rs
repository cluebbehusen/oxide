//! Bounded match execution with ordered, create-only evidence publication.

use super::*;
use std::num::NonZeroUsize;

/// Execution and evidence destinations shared by an evaluation batch.
pub struct EvaluationBatchOptions<'a> {
    /// Maximum ticks per leg.
    pub ticks: u64,
    /// Repeated per-unit stall limit; `None` disables early termination.
    pub stall_loop_limit: Option<u64>,
    /// Stable candidate identity recorded in every row.
    pub candidate: &'a str,
    /// Upper bound on concurrent matches, also bounded by available CPUs.
    pub jobs: NonZeroUsize,
    /// Optional compact JSONL index; required for replay or trace evidence.
    pub output: Option<&'a Path>,
    /// Optional ordered decision trace sidecar.
    pub trace_output: Option<&'a Path>,
}

/// Published evaluation rows, returned in input-plan order.
pub struct EvaluationBatchResult {
    /// Compact results, with exact replay destinations when requested.
    pub rows: Vec<EvaluationRow>,
    /// Number of decision trace records published.
    pub trace_rows: u64,
}

struct StagedLeg {
    row: EvaluationRow,
    replay: EvidenceBatch,
    trace: Option<StagedTrace>,
}

struct StagedTrace {
    // Keeps the private leg file alive until it is merged or the batch fails.
    _evidence: EvidenceBatch,
    path: PathBuf,
    rows: u64,
}

/// Evaluates independent legs and publishes their complete evidence set.
///
/// All destinations and execution identities are checked before dispatch. Each
/// worker stages its replay and streams its trace; completed results retain only
/// compact rows and file ownership. Memory for active replay payloads is bounded
/// by the worker count. Traces merge in plan order before create-only publication.
/// Normal errors clean staging and roll back publication; abrupt process death
/// has the same documented limitations as [`EvidenceBatch`].
pub fn evaluate_batch(
    plans: &[(EvaluationPlan, Option<PathBuf>)],
    options: &EvaluationBatchOptions<'_>,
) -> Result<EvaluationBatchResult> {
    validate_candidate(options.candidate)?;
    ensure!(options.ticks > 0, "tick limit must be positive");
    ensure!(
        options.stall_loop_limit != Some(0),
        "stall-loop limit must be positive or disabled"
    );
    ensure!(
        options.output.is_some()
            || (options.trace_output.is_none() && plans.iter().all(|(_, path)| path.is_none())),
        "replay and trace evidence require a compact evaluation index"
    );
    ensure_unique_execution_plans(plans.iter().map(|(plan, _)| plan))?;
    let mut destinations: Vec<_> = plans.iter().filter_map(|(_, path)| path.clone()).collect();
    destinations.extend(options.output.map(Path::to_path_buf));
    destinations.extend(options.trace_output.map(Path::to_path_buf));
    preflight_destinations(&destinations)?;

    let legs = crate::pool::fan_out_bounded(plans, options.jobs, |(plan, replay_path)| {
        let mut trace_evidence = EvidenceBatch::default();
        let mut trace_writer = options
            .trace_output
            .map(|path| trace_evidence.stage_trace_jsonl(path))
            .transpose()?;
        let (mut row, replay) = if let Some(writer) = &mut trace_writer {
            let (row, replay, _) = evaluate_plan_artifact_traced_with(
                plan,
                options.ticks,
                options.stall_loop_limit,
                options.candidate,
                |trace| writer.write_row(trace),
            )?;
            (row, replay)
        } else {
            evaluate_plan_artifact_with(
                plan,
                options.ticks,
                options.stall_loop_limit,
                options.candidate,
            )?
        };
        let trace = if let Some(writer) = trace_writer {
            let path = writer.staged.clone();
            let rows = writer.finish()?;
            Some(StagedTrace {
                _evidence: trace_evidence,
                path,
                rows,
            })
        } else {
            None
        };
        let mut evidence = EvidenceBatch::default();
        if let Some(path) = replay_path {
            evidence.stage_replay(&replay, path)?;
            row.replay = Some(path.display().to_string());
        }
        Ok(StagedLeg {
            row,
            replay: evidence,
            trace,
        })
    })?;

    let mut evidence = EvidenceBatch::default();
    let mut writer = options
        .trace_output
        .map(|path| evidence.stage_trace_jsonl(path))
        .transpose()?;
    let mut rows = Vec::with_capacity(legs.len());
    for mut leg in legs {
        if let (Some(writer), Some(trace)) = (&mut writer, &leg.trace) {
            let mut input =
                std::fs::File::open(&trace.path).context("opening completed leg trace")?;
            std::io::copy(
                &mut input,
                writer.writer.as_mut().expect("unfinished trace writer"),
            )
            .context("merging ordered leg trace")?;
            writer.rows = writer.rows.saturating_add(trace.rows);
        }
        evidence.staged.append(&mut leg.replay.staged);
        rows.push(leg.row);
        // The private trace is removed here; replay ownership moved to the batch.
    }
    let trace_rows = writer
        .map(EvaluationTraceWriter::finish)
        .transpose()?
        .unwrap_or(0);
    if let Some(path) = options.output {
        evidence.stage_jsonl(&rows, path)?;
    }
    evidence.publish()?;
    Ok(EvaluationBatchResult { rows, trace_rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oxide-eval-parallel-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn plans(dir: &Path) -> Vec<(EvaluationPlan, Option<PathBuf>)> {
        [73, 74]
            .into_iter()
            .flat_map(|seed| {
                configured_matchup_plans(
                    &Scenario::skirmish(),
                    seed,
                    ProfileMatchup::uniform(BotDifficulty::Standard, BotStance::Balanced),
                    8100,
                    true,
                    EvaluationFactionCell::Authored,
                    EvaluationGeometry::Authored,
                )
                .unwrap()
            })
            .enumerate()
            .map(|(i, plan)| (plan, Some(dir.join(format!("{i}.json")))))
            .collect()
    }

    #[test]
    fn batch_workers_preserve_rows_replays_and_trace_order() {
        let root = directory();
        let mut baseline = None;
        for jobs in [1, 2, 4] {
            let dir = root.join(jobs.to_string());
            let plans = plans(&dir);
            let index = dir.join("index.jsonl");
            let trace = dir.join("trace.jsonl");
            let result = evaluate_batch(
                &plans,
                &EvaluationBatchOptions {
                    ticks: 240,
                    stall_loop_limit: None,
                    candidate: "parallel-parity",
                    jobs: NonZeroUsize::new(jobs).unwrap(),
                    output: Some(&index),
                    trace_output: Some(&trace),
                },
            )
            .unwrap();
            assert!(result.trace_rows > 0);
            let published = std::fs::read_to_string(index).unwrap();
            assert_eq!(published.lines().count(), plans.len());
            let traces = std::fs::read(trace).unwrap();
            assert_eq!(
                traces.split(|&b| b == b'\n').count() - 1,
                result.trace_rows as usize
            );
            let replays: Vec<_> = plans
                .iter()
                .map(|(_, path)| std::fs::read(path.as_ref().unwrap()).unwrap())
                .collect();
            let mut rows = result.rows;
            for row in &mut rows {
                row.replay = None;
            }
            let evidence = (serde_json::to_vec(&rows).unwrap(), replays, traces);
            if let Some(ref baseline) = baseline {
                assert_eq!(&evidence, baseline);
            } else {
                baseline = Some(evidence);
            }
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), plans.len() + 2);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_leg_removes_completed_and_inflight_evidence() {
        for jobs in [1, 2] {
            let dir = directory();
            let mut plans = plans(&dir);
            plans[1].0.scenario.map.clear();
            let index = dir.join("index.jsonl");
            let trace = dir.join("trace.jsonl");
            let error = evaluate_batch(
                &plans,
                &EvaluationBatchOptions {
                    ticks: 24,
                    stall_loop_limit: None,
                    candidate: "failure-cleanup",
                    jobs: NonZeroUsize::new(jobs).unwrap(),
                    output: Some(&index),
                    trace_output: Some(&trace),
                },
            )
            .err()
            .expect("invalid scenario fails after dispatch");
            assert!(format!("{error:#}").contains("building bot evaluation scenario"));
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn existing_evidence_is_preserved_before_dispatch() {
        let dir = directory();
        let plans = plans(&dir);
        let index = dir.join("index.jsonl");
        let trace = dir.join("trace.jsonl");
        std::fs::write(&trace, b"previous evidence").unwrap();
        assert!(
            evaluate_batch(
                &plans,
                &EvaluationBatchOptions {
                    ticks: 24,
                    stall_loop_limit: None,
                    candidate: "preserve-evidence",
                    jobs: NonZeroUsize::new(4).unwrap(),
                    output: Some(&index),
                    trace_output: Some(&trace),
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&trace).unwrap(), b"previous evidence");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
