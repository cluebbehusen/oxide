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
            usize::try_from(result.trace_rows).unwrap()
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
