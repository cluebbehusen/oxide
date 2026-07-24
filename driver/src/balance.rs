//! The balance probe: bot-vs-bot matches across the shipped maps,
//! reporting cost-weighted army composition and its entropy — the
//! measuring stick the 0.10 balance review reads. Spam shows up as a
//! fat share and a thin entropy; an absent kind raises the weaker
//! question (it may just be hard to learn).

use anyhow::{Context, Result};
use oxide_kit::composition::{self, MatchComposition};
use oxide_sim::bot::Level;
use oxide_sim::scenario::BotConfig;
use std::collections::BTreeMap;

/// Runs the probe over every scenario in `dir` and prints the verdict;
/// `out` also lands the raw JSON for the record.
pub fn balance_probe(
    dir: &str,
    level: Level,
    skill: Option<u32>,
    blunder: u32,
    cadence: u64,
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

    let mut matches: Vec<MatchComposition> = Vec::new();
    for path in &paths {
        for offset in 0..seeds {
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
                            NeuralBot::with_profile(
                                oxide_sim::PlayerId(seat as u8),
                                cadence,
                                net.clone(),
                                skill.unwrap_or_else(|| level.skill()),
                                500,
                                player.faction,
                                blunder,
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
            eprintln!("  {} seed {} · {} ticks", m.scenario, m.seed, m.ticks);
            matches.push(m);
        }
    }

    let overall = composition::aggregate(&matches);
    println!(
        "\nBALANCE PROBE  ·  {} maps x {seeds} seeds  ·  level {level:?}",
        paths.len()
    );
    println!("cost-weighted mean army share (all seats):");
    let mut rows: Vec<(&String, &f64)> = overall.mean_share.iter().collect();
    rows.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    for (kind, share) in rows {
        println!("  {kind:<12} {:>5.1}%", share * 100.0);
    }
    println!(
        "mix entropy: {:.2} bits over {} seats",
        overall.entropy_bits, overall.seats
    );

    // Per-map aggregates flag geography-specific skews.
    let mut by_map: BTreeMap<String, Vec<MatchComposition>> = BTreeMap::new();
    for m in &matches {
        by_map
            .entry(m.scenario.clone())
            .or_default()
            .push(m.clone());
    }
    println!("\nper-map entropy:");
    for (name, ms) in &by_map {
        let agg = composition::aggregate(ms);
        let top = agg
            .mean_share
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(k, s)| format!("{k} {:.0}%", s * 100.0))
            .unwrap_or_default();
        println!("  {name:<18} {:.2} bits · top {top}", agg.entropy_bits);
    }

    if let Some(path) = out {
        let payload = serde_json::json!({
            "level": format!("{level:?}"),
            "seeds": seeds,
            "overall": overall,
            "matches": matches,
        });
        std::fs::write(path, serde_json::to_string_pretty(&payload)?)?;
        println!("\nraw record: {path}");
    }
    Ok(())
}
