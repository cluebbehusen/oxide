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
mod tests;
