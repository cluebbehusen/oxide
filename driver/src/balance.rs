//! The balance probe: bot-vs-bot matches across the shipped maps,
//! reporting cost-weighted and body-time-weighted army composition.
//! Value share is the balance lens; body-time share catches a cheap
//! unit dominating army presence while expensive specialists make the
//! value mix look varied.
//!
//! The report is cohorted because a pooled mean answers questions
//! nobody asked: the mean mix of two seats each spamming a different
//! kind looks diverse (the per-seat entropy floor is what catches
//! that), a capped stalemate's mix is evidence about a stalemate, and
//! a roster-wide skew hides inside a faction-blind average.

use anyhow::{Context, Result};
use oxide_kit::composition::{self, Aggregate, MatchComposition};
use oxide_sim::bot::Level;
use oxide_sim::scenario::BotConfig;

/// The candidate-probe dials that decompose the skill knob into its
/// parts — conditioning, blunder rate, think cadence — so experiments
/// can move one at a time. Shipped play always moves them together
/// through `Level`.
pub struct ProbeDials {
    /// Raw conditioning override (None: the level's own skill).
    pub skill: Option<u32>,
    /// Explicit blunder rate per mille (0: derive from skill).
    pub blunder: u32,
    /// Think cadence override (None: the level's own cadence).
    pub cadence: Option<u64>,
}

/// Runs the probe over every scenario in `dir` and prints the verdict;
/// `out` also lands the raw JSON for the record.
pub fn balance_probe(
    dir: &str,
    level: Level,
    dials: &ProbeDials,
    seeds: u64,
    max_ticks: u64,
    weights: Option<&str>,
    out: Option<&str>,
) -> Result<()> {
    // A candidate artifact probes exactly like the embedded one, just
    // with its net loaded from disk — the fun gate runs this before a
    // campaign checkpoint is ever embedded.
    let net = weights
        .map(|path| -> Result<oxide_sim::bot::QuantNet> {
            let json =
                std::fs::read_to_string(path).with_context(|| format!("reading weights {path}"))?;
            oxide_sim::bot::QuantNet::from_json(&json).map_err(|e| anyhow::anyhow!(e))
        })
        .transpose()?;
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {dir}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    anyhow::ensure!(!paths.is_empty(), "no scenarios under {dir}");

    // (map, seed offset). The pool returns results in job order, so the
    // record — and every cohort folded out of it — is ordered by map
    // and seed however many threads ran it.
    let jobs: Vec<(&std::path::PathBuf, u64)> = paths
        .iter()
        .flat_map(|path| (0..seeds).map(move |offset| (path, offset)))
        .collect();
    let matches: Vec<MatchComposition> = crate::pool::fan_out(&jobs, |&(path, offset)| {
        let mut sc = crate::runner::load_scenario(path.to_str().unwrap())?;
        sc.seed = 7_000 + offset;
        for player in sc.players.iter_mut() {
            player.bot = true;
            // Shipped-match configuration: the chosen level, with
            // the personality dealt from the scenario seed — the
            // game players actually fight.
            player.bot_config = Some(BotConfig {
                level,
                aggression: None,
            });
        }
        let m = match &net {
            None => composition::sample_match(&sc, max_ticks, 20)
                .with_context(|| format!("sampling {}", sc.name))?,
            Some(net) => {
                use oxide_sim::bot::NeuralBot;
                let mut bots: Vec<NeuralBot> = sc
                    .players
                    .iter()
                    .enumerate()
                    .map(|(seat, player)| {
                        // Exactly the shipped ladder profile unless a
                        // dial is explicitly overridden: the level's
                        // own cadence and the seed-dealt personality
                        // (the shipped dealing itself, not a copy).
                        // Probing a candidate at a flat cadence-16 /
                        // aggression-500 profile once gated a faster,
                        // blander bot than the one embedding ships.
                        let aggression = oxide_sim::bot::deal_aggression(
                            sc.seed,
                            oxide_sim::PlayerId(seat as u8),
                        );
                        NeuralBot::with_profile(
                            oxide_sim::PlayerId(seat as u8),
                            dials.cadence.unwrap_or_else(|| level.cadence()),
                            net.clone(),
                            dials.skill.unwrap_or_else(|| level.skill()),
                            aggression,
                            player.faction,
                            dials.blunder,
                            sc.seed,
                        )
                    })
                    .collect();
                composition::sample_driven(&sc, max_ticks, 20, |state| {
                    let mut commands = Vec::new();
                    for bot in bots.iter_mut() {
                        commands.extend(bot.act(state));
                    }
                    state.tick(&commands);
                })
                .with_context(|| format!("sampling {}", sc.name))?
            }
        };
        eprintln!(
            "  {} seed {} · {} ticks · {}",
            m.scenario,
            m.seed,
            m.ticks,
            if m.capped { "capped" } else { "decided" }
        );
        Ok(m)
    })?;

    // Provenance: a composition table is evidence about ONE artifact,
    // and a candidate's file name outlives neither the campaign nor
    // the run directory. The digest does.
    let (artifact, digest) = match (&net, weights) {
        (Some(net), Some(path)) => (path.to_string(), net.digest()),
        _ => (
            "embedded ladder".to_string(),
            oxide_sim::bot::QuantNet::ladder().digest(),
        ),
    };

    let overall = composition::aggregate(&matches);
    let decided = composition::aggregate_where(&matches, |m, _| !m.capped);
    println!(
        "\nBALANCE PROBE  ·  {} maps x {seeds} seeds  ·  level {level:?}",
        paths.len()
    );
    println!("artifact: {artifact} · digest {digest:016x}");
    println!(
        "{} matches · {} decided / {} capped by the {max_ticks}-tick cap ({:.1}% censored)",
        overall.matches,
        overall.decided,
        overall.capped,
        censored(&overall)
    );
    println!("\ncost-weighted mean army share (all seats):");
    let mut rows: Vec<(&String, &f64)> = overall.mean_share.iter().collect();
    rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "mix entropy: {:.2} bits over {} seats",
        overall.entropy_bits, overall.seats
    );
    if let Some(spread) = &overall.seat_entropy {
        // The mean mix cannot see a single collapsed seat: two seats
        // each spamming a different kind average to a diverse-looking
        // mix. p10 is where that seat shows up.
        println!(
            "per-seat entropy: mean {:.2} · p10 {:.2} · p25 {:.2} · median {:.2} bits",
            spread.mean, spread.p10, spread.p25, spread.median
        );
    }
    println!("\nbody-time mean army share (all seats):");
    let mut count_rows: Vec<(&String, &f64)> = overall.mean_count_share.iter().collect();
    count_rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in count_rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "count mix entropy: {:.2} bits over {} seats",
        overall.count_entropy_bits, overall.seats
    );
    if let (Some(entropy), Some(dominance)) =
        (&overall.seat_count_entropy, &overall.seat_count_dominance)
    {
        println!(
            "per-seat count entropy: mean {:.2} · p10 {:.2} · median {:.2} bits",
            entropy.mean, entropy.p10, entropy.median
        );
        println!(
            "largest body-time share per seat: mean {:.1}% · p90 {:.1}% · max {:.1}%",
            dominance.mean * 100.0,
            dominance.p90 * 100.0,
            dominance.max * 100.0
        );
    }

    // What was finished, not just what was fielded: a roster that never
    // stands a Fabricator never had the advanced kinds to choose from.
    println!("\nfinished buildings per seat (mean · share of seats that stood one):");
    for (kind, mean) in &overall.mean_buildings {
        let reach = overall
            .seats_with_building
            .get(kind)
            .copied()
            .unwrap_or_default();
        println!("  {kind:<12} {mean:>5.2} · {:>5.1}%", reach * 100.0);
    }

    // A capped stalemate's army mix is evidence about a stalemate. The
    // decided cohort is the one a promotion gate should read.
    println!("\ndecided matches only:");
    print_cohort_header();
    print_cohort_row("decided", &decided);

    for (title, cohorts) in [
        (
            "by faction",
            composition::aggregate_by(&matches, composition::by_faction),
        ),
        (
            "by map class",
            composition::aggregate_by(&matches, composition::by_pace),
        ),
        (
            "by outcome",
            composition::aggregate_by(&matches, composition::by_outcome),
        ),
        (
            "per map",
            composition::aggregate_by(&matches, composition::by_scenario),
        ),
    ] {
        println!("\n{title}:");
        print_cohort_header();
        for (name, agg) in &cohorts {
            print_cohort_row(name, agg);
        }
    }

    if let Some(path) = out {
        let payload = serde_json::json!({
            // Bumped whenever a consumer (tools/train/fun_gate.py) would
            // need to read this file differently.
            "schema": 3,
            "level": format!("{level:?}"),
            "artifact": artifact,
            "digest": format!("{digest:016x}"),
            "seeds": seeds,
            "max_ticks": max_ticks,
            "overall": overall,
            "decided": decided,
            "cohorts": {
                "faction": composition::aggregate_by(&matches, composition::by_faction),
                "pace": composition::aggregate_by(&matches, composition::by_pace),
                "outcome": composition::aggregate_by(&matches, composition::by_outcome),
                "map": composition::aggregate_by(&matches, composition::by_scenario),
            },
            "matches": matches,
        });
        std::fs::write(path, serde_json::to_string_pretty(&payload)?)?;
        println!("\nraw record: {path}");
    }
    Ok(())
}

/// Censored share of a cohort's matches, in percent.
fn censored(agg: &Aggregate) -> f64 {
    if agg.matches == 0 {
        0.0
    } else {
        agg.capped as f64 * 100.0 / agg.matches as f64
    }
}

fn print_cohort_header() {
    println!(
        "  {:<20} {:>6} {:>6} {:>7} {:>7} {:>7} {:>7} {:>7}  top body",
        "cohort", "seats", "cens", "val H", "val p10", "body H", "body p10", "body p90"
    );
}

/// One cohort line: how much of it there is, how censored it is, how
/// diverse the mean mix is, and — the reading a mean cannot give — how
/// the thinnest seats scored.
fn print_cohort_row(name: &str, agg: &Aggregate) {
    let top = agg
        .mean_count_share
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, s)| format!("{k} {:.0}%", s * 100.0))
        .unwrap_or_else(|| "-".to_string());
    // An empty cohort has no entropy; printing 0.00 would read as the
    // one thing this table exists to flag.
    let cell = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
    let value_spread = agg.seat_entropy.as_ref();
    let count_spread = agg.seat_count_entropy.as_ref();
    let dominance = agg.seat_count_dominance.as_ref();
    println!(
        "  {name:<20} {:>6} {:>5.1}% {:>7} {:>7} {:>7} {:>7} {:>7}  {top}",
        agg.seats,
        censored(agg),
        cell(value_spread.map(|_| agg.entropy_bits)),
        cell(value_spread.map(|s| s.p10)),
        cell(count_spread.map(|_| agg.count_entropy_bits)),
        cell(count_spread.map(|s| s.p10)),
        cell(dominance.map(|s| s.p90)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `--out` payload is a contract with `tools/train/fun_gate.py`,
    /// which reads it by key and cannot fail loudly on a reshape — so
    /// the keys it reads are pinned here, on the Rust side of the seam.
    /// A few ticks per map is enough: this is about the shape.
    #[test]
    fn the_probe_payload_carries_what_the_fun_gate_reads() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios");
        let out = std::env::temp_dir().join(format!("oxide-probe-{}.json", std::process::id()));
        balance_probe(
            dir,
            Level::Easy,
            &ProbeDials {
                skill: None,
                blunder: 0,
                cadence: None,
            },
            1,
            40,
            None,
            out.to_str(),
        )
        .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        std::fs::remove_file(&out).ok();
        assert_eq!(payload["schema"], 3);
        assert!(payload["overall"]["matches"].as_u64().unwrap() >= 25);
        // Nothing decides in 40 ticks, so the gate's own cohort is
        // empty — which the gate must be able to SEE, not infer.
        assert_eq!(payload["decided"]["seats"], 0);
        assert_eq!(payload["decided"]["matches"], 0);
        assert!(payload["decided"]["mean_share"].is_object());
        assert!(payload["decided"]["entropy_bits"].is_number());
        assert!(payload["overall"]["seat_entropy"]["p10"].is_number());
        assert!(payload["decided"]["mean_count_share"].is_object());
        assert!(payload["decided"]["count_entropy_bits"].is_number());
        assert!(payload["overall"]["seat_count_entropy"]["p10"].is_number());
        assert!(payload["overall"]["seat_count_dominance"]["p90"].is_number());
        for cohort in ["faction", "pace", "outcome", "map"] {
            assert!(
                payload["cohorts"][cohort].is_object(),
                "{cohort} cohort is reported"
            );
        }
        assert_eq!(payload["cohorts"]["faction"]["ferrous"]["capped"], 25);
        let first = &payload["matches"][0];
        assert!(first["capped"].as_bool().unwrap() && first["result"].is_null());
        assert!(first["last_progress_tick"].as_u64().unwrap() > 0);
    }
}
