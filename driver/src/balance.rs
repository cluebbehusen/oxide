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
use oxide_sim::bot::{Level, NAMED_VARIANT_COUNT, resolve_bot_profiles};
use oxide_sim::scenario::{BotConfig, NamedStyle};

/// Candidate-probe overrides for conditioning, hesitation, and think
/// cadence. Without a raw conditioning or hesitation override,
/// candidates use the same resolved named profile and level handicap
/// a shipped match would. Without `--weights` at all, the probe runs
/// the Overseer — the scripted QA anchor — in every seat.
pub struct ProbeDials {
    /// Raw conditioning override. `None` keeps the resolved named
    /// profile.
    pub skill: Option<u32>,
    /// Raw aggression override. `None` keeps the seed-resolved named
    /// style and variant.
    pub aggression: Option<u32>,
    /// Exact named style. `None` keeps the scenario seed's deterministic
    /// profile deal.
    pub style: Option<NamedStyle>,
    /// Curated variant within `style`, 0..=2. `None` keeps the named
    /// variant deal.
    pub variant: Option<u8>,
    /// Exact hesitation rate per mille. Supplying this, including zero,
    /// opts into a raw experimental profile; `None` keeps the named
    /// level's handicap.
    pub blunder: Option<u32>,
    /// Think cadence override (None: the level's own cadence).
    pub cadence: Option<u64>,
}

impl ProbeDials {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.style.is_some() || self.variant.is_none(),
            "--variant requires --style"
        );
        anyhow::ensure!(
            self.variant
                .is_none_or(|variant| variant < NAMED_VARIANT_COUNT),
            "--variant must be 0, 1, or 2"
        );
        anyhow::ensure!(
            self.style.is_none()
                || (self.skill.is_none() && self.aggression.is_none() && self.blunder.is_none()),
            "--style is mutually exclusive with --skill, --aggression, and --blunder"
        );
        Ok(())
    }

    fn uses_manual_profile(&self) -> bool {
        self.skill.is_some() || self.blunder.is_some()
    }

    fn uses_raw_profile(&self) -> bool {
        self.uses_manual_profile() || self.aggression.is_some()
    }
}

fn candidate_bot(
    dials: &ProbeDials,
    level: Level,
    player: oxide_sim::PlayerId,
    scenario_seed: u64,
    profile: oxide_sim::bot::ResolvedBotProfile,
    faction: oxide_sim::Faction,
    net: oxide_sim::bot::QuantNet,
) -> oxide_sim::bot::NeuralBot {
    use oxide_sim::bot::NeuralBot;

    if dials.uses_manual_profile() {
        let aggression = dials
            .aggression
            .unwrap_or_else(|| oxide_sim::bot::deal_aggression(scenario_seed, player));
        NeuralBot::with_profile_hesitation(
            player,
            dials.cadence.unwrap_or_else(|| level.cadence()),
            net,
            dials.skill.unwrap_or_else(|| level.skill()),
            aggression,
            faction,
            dials.blunder,
            scenario_seed,
        )
    } else if let Some(cadence) = dials.cadence {
        NeuralBot::ladder_resolved_with_net_at_cadence(
            player,
            scenario_seed,
            profile,
            faction,
            net,
            cadence,
        )
    } else {
        NeuralBot::ladder_resolved_with_net(player, scenario_seed, profile, faction, net)
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
    dials.validate()?;
    // A candidate artifact probes with its net loaded from disk — the
    // fun gate runs this before a campaign checkpoint is ever promoted.
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
                style: dials.style,
                variant: dials.variant,
                team_role: None,
            });
        }
        let m = match &net {
            // No candidate weights: the Overseer plays every seat —
            // `sample_match` seats it per bot seat, and the probe just
            // flipped every seat to a bot.
            None => composition::sample_match(&sc, max_ticks, 20)
                .with_context(|| format!("sampling {}", sc.name))?,
            Some(net) => {
                let profiles = resolve_bot_profiles(&sc)
                    .with_context(|| format!("resolving bot profiles for {}", sc.name))?;
                let mut bots: Vec<oxide_sim::bot::NeuralBot> = sc
                    .players
                    .iter()
                    .enumerate()
                    .map(|(seat, player)| {
                        // Exactly the scenario-resolved shipped profile
                        // unless raw skill or hesitation is explicitly
                        // overridden. A cadence-only probe retains the
                        // named style, variant, team role, and level
                        // hesitation.
                        candidate_bot(
                            dials,
                            level,
                            oxide_sim::PlayerId(seat as u8),
                            sc.seed,
                            profiles[seat].expect("configured probe bot has a profile"),
                            player.faction,
                            net.clone(),
                        )
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
    // the run directory. The digest does. A weights-less run is the
    // scripted Overseer and says so.
    let (artifact, digest) = match (&net, weights) {
        (Some(net), Some(path)) => (path.to_string(), format!("{:016x}", net.digest())),
        _ => ("scripted overseer".to_string(), "overseer".to_string()),
    };
    let profile = if net.is_none() {
        "overseer"
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
            "schema": 9,
            "level": format!("{level:?}"),
            "artifact": artifact,
            "digest": digest,
            "profile": profile,
            "seeds": seeds,
            "max_ticks": max_ticks,
            "dials": {
                "skill": dials.skill,
                "aggression": dials.aggression,
                "style": dials.style.map(style_slug),
                "variant": dials.variant,
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

fn style_slug(style: NamedStyle) -> &'static str {
    match style {
        NamedStyle::Turtle => "turtle",
        NamedStyle::Balanced => "balanced",
        NamedStyle::Aggressive => "aggressive",
    }
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
    use oxide_sim::bot::{NeuralBot, QuantNet};
    use oxide_sim::scenario::TeamRole;

    fn empty_dials() -> ProbeDials {
        ProbeDials {
            skill: None,
            aggression: None,
            style: None,
            variant: None,
            blunder: None,
            cadence: None,
        }
    }

    #[test]
    fn exact_raw_overrides_are_distinct_from_the_candidate_ladder_profile() {
        let mut dials = empty_dials();
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

        dials.skill = None;
        dials.aggression = Some(550);
        assert!(
            dials.uses_raw_profile(),
            "an exact aggression override bypasses named profile facets"
        );
    }

    #[test]
    fn named_profile_selectors_refuse_ambiguous_raw_controls() {
        let mut dials = empty_dials();
        dials.variant = Some(1);
        assert!(
            dials
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires")
        );

        dials.style = Some(NamedStyle::Turtle);
        dials.variant = Some(NAMED_VARIANT_COUNT);
        assert!(
            dials
                .validate()
                .unwrap_err()
                .to_string()
                .contains("0, 1, or 2")
        );

        for configure in [
            |dials: &mut ProbeDials| dials.skill = Some(700),
            |dials: &mut ProbeDials| dials.aggression = Some(550),
            |dials: &mut ProbeDials| dials.blunder = Some(0),
        ] {
            let mut conflicting = empty_dials();
            conflicting.style = Some(NamedStyle::Balanced);
            conflicting.variant = Some(1);
            configure(&mut conflicting);
            assert!(
                conflicting
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("mutually exclusive")
            );
        }
    }

    #[test]
    fn candidate_named_selector_receives_the_runtime_resolved_team_profile() {
        let mut scenario = crate::runner::load_scenario(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/open-quarry.json"
        ))
        .unwrap();
        scenario.seed = 31;
        for player in &mut scenario.players {
            player.bot = true;
            player.bot_config = Some(BotConfig {
                level: Level::Medium,
                aggression: None,
                style: Some(NamedStyle::Turtle),
                variant: Some(1),
                team_role: None,
            });
        }
        let profiles = resolve_bot_profiles(&scenario).unwrap();
        let (seat, profile) = profiles
            .iter()
            .enumerate()
            .find_map(|(seat, profile)| {
                profile
                    .filter(|profile| profile.team_role != TeamRole::Generalist)
                    .map(|profile| (seat, profile))
            })
            .expect("a team map resolves a specialized runtime role");
        assert_eq!(profile.style, Some(NamedStyle::Turtle));
        assert_eq!(profile.variant, Some(1));

        let dials = ProbeDials {
            style: Some(NamedStyle::Turtle),
            variant: Some(1),
            cadence: Some(11),
            ..empty_dials()
        };
        let net = QuantNet::from_json(include_str!("../tests/fixtures/tiny_policy_v9.json"))
            .expect("the committed fixture artifact parses");
        let player = oxide_sim::PlayerId(seat as u8);
        let faction = scenario.players[seat].faction;
        let mut actual = candidate_bot(
            &dials,
            Level::Medium,
            player,
            scenario.seed,
            profile,
            faction,
            net.clone(),
        );
        let mut expected = NeuralBot::ladder_resolved_with_net_at_cadence(
            player,
            scenario.seed,
            profile,
            faction,
            net,
            11,
        );
        let mut state = scenario.build().unwrap();
        for _ in 0..200 {
            let actual_commands = actual.act(&state);
            let expected_commands = expected.act(&state);
            assert_eq!(actual_commands, expected_commands);
            state.tick(&expected_commands);
        }
    }

    #[test]
    fn candidate_probe_accepts_the_scenario_resolved_named_profile_slate() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios");
        let weights = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tiny_policy_v9.json"
        );
        let out =
            std::env::temp_dir().join(format!("oxide-candidate-probe-{}.json", std::process::id()));
        balance_probe(
            dir,
            Level::Easy,
            &ProbeDials {
                skill: None,
                aggression: None,
                style: Some(NamedStyle::Balanced),
                variant: Some(1),
                blunder: None,
                cadence: Some(11),
            },
            1,
            1,
            Some(weights),
            out.to_str(),
        )
        .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        std::fs::remove_file(&out).ok();

        assert_eq!(payload["schema"], 9);
        assert_eq!(payload["profile"], "ladder");
        assert_eq!(payload["dials"]["style"], "balanced");
        assert_eq!(payload["dials"]["variant"], 1);
        assert_eq!(payload["dials"]["cadence"], 11);
        assert_ne!(payload["digest"], "overseer");
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
            // This is a payload-shape test: no weights and no dials
            // selects the scripted Overseer default, which keeps it
            // independent of any candidate artifact.
            &empty_dials(),
            1,
            40,
            None,
            out.to_str(),
        )
        .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        std::fs::remove_file(&out).ok();
        assert_eq!(payload["schema"], 9);
        // One match per shipped map: the roster is mid-rework for 0.15,
        // so the floor tracks the roster instead of pinning a count.
        let shipped = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios"))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            })
            .count() as u64;
        assert!(payload["overall"]["matches"].as_u64().unwrap() >= shipped);
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
        assert!(payload["dials"]["style"].is_null());
        assert!(payload["dials"]["variant"].is_null());
        assert!(payload["overall"]["seat_count_dominance"]["p90"].is_number());
        for cohort in ["faction", "pace", "outcome", "map"] {
            assert!(
                payload["cohorts"][cohort].is_object(),
                "{cohort} cohort is reported"
            );
        }
        assert_eq!(
            payload["cohorts"]["faction"]["ferrous"]["capped"], shipped,
            "every map's single probe match caps at 40 ticks"
        );
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
        // The fun-metric fields the retrain campaign reads: fight
        // rhythm, the decided moment, and contested-economy tenure.
        assert!(first["fight_windows"].is_u64());
        assert!(first["fight_share"].is_number());
        assert!(first["longest_lull_ticks"].is_u64());
        assert!(first["advantage_tick"].is_null() || first["advantage_tick"].is_u64());
        assert!(first["advantage_team"].is_null() || first["advantage_team"].is_u64());
        assert!(
            first["extractor_hold_share"].as_array().is_some_and(|per| {
                per.len() == first["factions"].as_array().unwrap().len()
                    && per.iter().all(serde_json::Value::is_number)
            }),
            "extractor hold share is per-seat"
        );
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
