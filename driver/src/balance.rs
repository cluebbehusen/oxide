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
use oxide_sim::bot::{Difficulty, Level};
use oxide_sim::scenario::BotConfig;

/// Candidate-probe overrides for conditioning, hesitation, and think
/// cadence. Without a raw conditioning or hesitation override,
/// candidates use the same strategy-specific policy condition and
/// level handicap as the shipped ladder.
pub struct ProbeDials {
    /// Run the scripted utility controller at this tier instead of the
    /// neural ladder.
    pub scripted: Option<Difficulty>,
    /// Raw conditioning override. `None` keeps the ladder's
    /// strategy-specific policy condition.
    pub skill: Option<u32>,
    /// Personality override (None: deal the shipped seed-derived value).
    pub aggression: Option<u32>,
    /// Exact hesitation rate per mille. Supplying this, including zero,
    /// opts into a raw experimental profile; `None` keeps the named
    /// level's handicap.
    pub blunder: Option<u32>,
    /// Think cadence override (None: the level's own cadence).
    pub cadence: Option<u64>,
}

impl ProbeDials {
    fn uses_raw_profile(&self) -> bool {
        self.skill.is_some() || self.blunder.is_some()
    }
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
    anyhow::ensure!(
        dials.scripted.is_none() || weights.is_none(),
        "--scripted-tier and --weights are mutually exclusive"
    );
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
            // the personality dealt from the scenario seed unless the
            // probe explicitly isolates one personality.
            player.bot_config = Some(BotConfig {
                level,
                aggression: dials.aggression,
            });
        }
        let m = match (dials.scripted, &net) {
            (Some(tier), _) => {
                use oxide_sim::bot::Brain;
                let mut bots: Vec<Brain> = sc
                    .players
                    .iter()
                    .enumerate()
                    .map(|(seat, _)| {
                        Brain::for_tier(oxide_sim::PlayerId(seat as u8), sc.seed, tier)
                    })
                    .collect();
                composition::sample_driven(&sc, max_ticks, 20, |state| {
                    let mut commands = Vec::new();
                    for bot in bots.iter_mut() {
                        commands.extend(bot.act(state));
                    }
                    state.tick(&commands)
                })
                .with_context(|| format!("sampling {}", sc.name))?
            }
            (None, None) => composition::sample_match(&sc, max_ticks, 20)
                .with_context(|| format!("sampling {}", sc.name))?,
            (None, Some(net)) => {
                use oxide_sim::bot::NeuralBot;
                let mut bots: Vec<NeuralBot> = sc
                    .players
                    .iter()
                    .enumerate()
                    .map(|(seat, player)| {
                        // Exactly the shipped ladder profile unless a
                        // raw skill or hesitation dial is explicitly
                        // overridden. A cadence-only probe still keeps
                        // the named level's hesitation and
                        // strategy-conditioned policy skill.
                        let aggression = dials.aggression.unwrap_or_else(|| {
                            oxide_sim::bot::deal_aggression(
                                sc.seed,
                                oxide_sim::PlayerId(seat as u8),
                            )
                        });
                        if dials.uses_raw_profile() {
                            NeuralBot::with_profile_hesitation(
                                oxide_sim::PlayerId(seat as u8),
                                dials.cadence.unwrap_or_else(|| level.cadence()),
                                net.clone(),
                                dials.skill.unwrap_or_else(|| level.skill()),
                                aggression,
                                player.faction,
                                dials.blunder,
                                sc.seed,
                            )
                        } else if let Some(cadence) = dials.cadence {
                            NeuralBot::ladder_with_net_at_cadence(
                                oxide_sim::PlayerId(seat as u8),
                                sc.seed,
                                level,
                                Some(aggression),
                                player.faction,
                                net.clone(),
                                cadence,
                            )
                        } else {
                            NeuralBot::ladder_with_net(
                                oxide_sim::PlayerId(seat as u8),
                                sc.seed,
                                level,
                                Some(aggression),
                                player.faction,
                                net.clone(),
                            )
                        }
                    })
                    .collect();
                composition::sample_driven(&sc, max_ticks, 20, |state| {
                    let mut commands = Vec::new();
                    for bot in bots.iter_mut() {
                        commands.extend(bot.act(state));
                    }
                    state.tick(&commands)
                })
                .with_context(|| format!("sampling {}", sc.name))?
            }
        };
        eprintln!(
            "  {} seed {} · {} ticks · {}",
            m.scenario,
            m.seed,
            m.ticks,
            match_status(&m)
        );
        Ok(m)
    })?;

    // Provenance: a composition table is evidence about ONE artifact,
    // and a candidate's file name outlives neither the campaign nor
    // the run directory. The digest does.
    let (artifact, digest) = match (dials.scripted, &net, weights) {
        (Some(tier), _, _) => (format!("scripted {tier:?}"), "scripted".to_string()),
        (None, Some(net), Some(path)) => (path.to_string(), format!("{:016x}", net.digest())),
        _ => (
            "embedded ladder".to_string(),
            format!("{:016x}", oxide_sim::bot::QuantNet::ladder().digest()),
        ),
    };
    let profile = if dials.scripted.is_some() {
        "scripted"
    } else if dials.uses_raw_profile() {
        "raw"
    } else {
        "ladder"
    };

    let overall = composition::aggregate(&matches);
    let decided = composition::aggregate_where(&matches, |m, _| !m.capped);
    println!(
        "\nBALANCE PROBE  ·  {} maps x {seeds} seeds  ·  level {level:?}",
        paths.len()
    );
    println!("artifact: {artifact} · digest {digest}");
    println!(
        "{} matches · {} decided / {} capped by the {max_ticks}-tick cap ({:.1}% censored)",
        overall.matches,
        overall.decided,
        overall.capped,
        censored(&overall)
    );
    println!("\nall-unit cost-weighted mean roster share (diagnostic):");
    let mut rows: Vec<(&String, &f64)> = overall.mean_share.iter().collect();
    rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "all-unit mix entropy: {:.2} bits over {} seats",
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
    println!("\nall-unit body-time mean roster share (diagnostic):");
    let mut count_rows: Vec<(&String, &f64)> = overall.mean_count_share.iter().collect();
    count_rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in count_rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "all-unit count mix entropy: {:.2} bits over {} seats",
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

    println!("\ncompetitive-lifetime combat value share:");
    let mut combat_rows: Vec<(&String, &f64)> = overall.mean_combat_share.iter().collect();
    combat_rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in combat_rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "combat mix entropy: {:.2} bits over {} seats",
        overall.combat_entropy_bits, overall.combat_seats
    );
    println!("\ncompetitive-lifetime combat body-time share:");
    let mut combat_count_rows: Vec<(&String, &f64)> =
        overall.mean_combat_count_share.iter().collect();
    combat_count_rows.sort_by(|a, b| b.1.total_cmp(a.1));
    for (kind, share) in combat_count_rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    if let (Some(entropy), Some(dominance)) = (
        &overall.seat_combat_count_entropy,
        &overall.seat_combat_count_dominance,
    ) {
        println!(
            "combat body entropy: {:.2} bits · per-seat p10 {:.2} · \
             largest-share p90 {:.1}%",
            overall.combat_count_entropy_bits,
            entropy.p10,
            dominance.p90 * 100.0
        );
    }

    // What was finished, not just what was fielded: a roster that never
    // stands a Fabricator never had the advanced kinds to choose from.
    println!("\nfinished buildings per all-unit seat (all-time diagnostic):");
    for (kind, mean) in &overall.mean_buildings {
        let reach = overall
            .seats_with_building
            .get(kind)
            .copied()
            .unwrap_or_default();
        println!("  {kind:<12} {mean:>5.2} · {:>5.1}%", reach * 100.0);
    }
    println!("\ncompetitive-lifetime buildings (mean · reach):");
    for (kind, mean) in &overall.competitive_mean_buildings {
        let reach = overall
            .competitive_seats_with_building
            .get(kind)
            .copied()
            .unwrap_or_default();
        println!("  {kind:<12} {mean:>5.2} · {:>5.1}%", reach * 100.0);
    }

    // Promotion judges every seat's competitive lifetime. Losing seats
    // keep their pre-defeat history; the sampler stops their combat
    // clock once they resign or lose their completed Foundry.
    println!("\nall competitive lifetimes:");
    print_cohort_header();
    print_cohort_row("overall", &overall);

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
            "schema": 6,
            "level": format!("{level:?}"),
            "artifact": artifact,
            "digest": digest,
            "profile": profile,
            "seeds": seeds,
            "max_ticks": max_ticks,
            "dials": {
                "scripted": dials.scripted.map(|tier| format!("{tier:?}")),
                "skill": dials.skill,
                "aggression": dials.aggression,
                "blunder": dials.blunder.unwrap_or(0),
                "cadence": dials.cadence,
            },
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

fn match_status(m: &MatchComposition) -> String {
    if !m.capped {
        return "decided".to_string();
    }
    let seats = m
        .final_economy
        .seats
        .iter()
        .enumerate()
        .map(|(index, seat)| {
            format!(
                "p{index}:b{} h{}+{}q r{} f{}{}",
                seat.bank_scrap,
                seat.living_harvesters,
                seat.queued_harvesters,
                seat.completed_reclaimers,
                seat.living_foundries,
                if seat.resigned { " resigned" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "capped · combat age {}t · economy age {}t · roster age {}t · salvage {} · {seats}",
        m.ticks.saturating_sub(m.activity.last_combat_tick),
        m.ticks.saturating_sub(m.activity.last_economy_tick),
        m.ticks.saturating_sub(m.last_progress_tick),
        m.final_economy.remaining_map_salvage
    )
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
        .mean_combat_count_share
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, s)| format!("{k} {:.0}%", s * 100.0))
        .unwrap_or_else(|| "-".to_string());
    // An empty cohort has no entropy; printing 0.00 would read as the
    // one thing this table exists to flag.
    let cell = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
    let value_spread = agg.seat_combat_entropy.as_ref();
    let count_spread = agg.seat_combat_count_entropy.as_ref();
    let dominance = agg.seat_combat_count_dominance.as_ref();
    println!(
        "  {name:<20} {:>6} {:>5.1}% {:>7} {:>7} {:>7} {:>7} {:>7}  {top}",
        agg.combat_seats,
        censored(agg),
        cell(value_spread.map(|_| agg.combat_entropy_bits)),
        cell(value_spread.map(|s| s.p10)),
        cell(count_spread.map(|_| agg.combat_count_entropy_bits)),
        cell(count_spread.map(|s| s.p10)),
        cell(dominance.map(|s| s.p90)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_raw_overrides_are_distinct_from_the_candidate_ladder_profile() {
        let mut dials = ProbeDials {
            scripted: None,
            skill: None,
            aggression: None,
            blunder: None,
            cadence: None,
        };
        assert!(!dials.uses_raw_profile());

        dials.blunder = Some(0);
        assert!(
            dials.uses_raw_profile(),
            "an explicit zero requests exact zero hesitation"
        );

        dials.blunder = None;
        dials.skill = Some(Level::Expert.skill());
        assert!(
            dials.uses_raw_profile(),
            "even a numerically familiar skill is an explicit raw profile"
        );
    }

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
                // This is a payload-shape test, so use the scripted
                // controller and keep it independent of whichever
                // generated ladder artifact is mid-regeneration.
                scripted: Some(Difficulty::Scrapheap),
                skill: None,
                aggression: None,
                blunder: None,
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
        assert_eq!(payload["schema"], 6);
        assert!(payload["overall"]["matches"].as_u64().unwrap() >= 25);
        // Nothing decides in 40 ticks, so the diagnostic decided cohort
        // is empty — which consumers must be able to see, not infer.
        assert_eq!(payload["decided"]["seats"], 0);
        assert_eq!(payload["decided"]["matches"], 0);
        assert!(payload["decided"]["mean_share"].is_object());
        assert!(payload["decided"]["entropy_bits"].is_number());
        assert!(payload["overall"]["seat_entropy"]["p10"].is_number());
        assert!(payload["decided"]["mean_count_share"].is_object());
        assert!(payload["decided"]["count_entropy_bits"].is_number());
        assert!(payload["overall"]["seat_count_entropy"]["p10"].is_number());
        // The gate reads only these parallel combat fields. The old
        // all-unit maps above remain diagnostics and include Harvesters.
        assert!(payload["overall"]["combat_seats"].as_u64().unwrap() > 0);
        assert!(payload["overall"]["mean_combat_share"].is_object());
        assert!(payload["overall"]["combat_entropy_bits"].is_number());
        assert!(payload["overall"]["seat_combat_entropy"]["p10"].is_number());
        assert!(payload["overall"]["mean_combat_count_share"].is_object());
        assert!(payload["overall"]["combat_count_entropy_bits"].is_number());
        assert!(payload["overall"]["seat_combat_count_entropy"]["p10"].is_number());
        assert!(payload["overall"]["seat_combat_count_dominance"]["p90"].is_number());
        assert!(payload["overall"]["competitive_seats_with_building"].is_object());
        assert!(payload["dials"]["aggression"].is_null());
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
        for key in [
            "last_combat_tick",
            "last_economy_tick",
            "attack_hits",
            "turret_shots",
            "shell_shots",
            "deliveries",
            "delivered_scrap",
            "units_trained",
            "buildings_completed",
        ] {
            assert!(first["activity"][key].is_u64(), "{key} is reported");
        }
        assert!(
            first["final_economy"]["remaining_map_salvage"]
                .as_u64()
                .unwrap()
                > 0
        );
        let economy_seats = first["final_economy"]["seats"].as_array().unwrap();
        assert_eq!(economy_seats.len(), 2);
        for seat in economy_seats {
            for key in [
                "resigned",
                "recovery_income_active",
                "bank_scrap",
                "living_harvesters",
                "queued_harvesters",
                "completed_reclaimers",
                "living_foundries",
            ] {
                if matches!(key, "resigned" | "recovery_income_active") {
                    assert!(seat[key].is_boolean(), "{key} is reported per seat");
                } else {
                    assert!(seat[key].is_u64(), "{key} is reported per seat");
                }
            }
        }
        assert!(first["seats"][0]["harvester"].is_number());
        assert!(first["combat_seats"][0]["harvester"].is_null());
    }
}
