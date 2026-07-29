//! The training loop's other half: episodes over stdio.
//!
//! `oxide-driver gym` speaks newline-delimited JSON on stdin/stdout so
//! a trainer (see `tools/train/`) can drive [`GymBot`] episodes without
//! linking Rust. `control` names the externally-driven seats: one seat
//! against a scripted tier for curriculum and evaluation, or both
//! seats for self-play and league play — every decision tick then
//! carries features and masks for each controlled seat, and `step`
//! takes one action per controlled seat, in the same order.
//! Determinism holds the whole way down: same seed and same actions
//! replay the same match, which is what makes rollouts auditable.

use anyhow::{Context, Result, bail};
use oxide_sim::bot::{ACTION_COUNT, Action, Brain, Difficulty, FEATURE_COUNT, GYM_VERSION, GymBot};
use oxide_sim::scenario::Scenario;
use oxide_sim::state::GameResult;
use oxide_sim::{Faction, PlayerId, State};
use serde::Deserialize;
use std::io::{BufRead, Write};

/// Ordered west/east faction pair for a two-seat neural cup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuelFactions([Faction; 2]);

impl DuelFactions {
    /// Compact ordered pair used by the CLI and JSON provenance.
    pub fn code(self) -> &'static str {
        match self.0 {
            [Faction::Ferrous, Faction::Ferrous] => "ff",
            [Faction::Ferrous, Faction::Cupric] => "fc",
            [Faction::Cupric, Faction::Ferrous] => "cf",
            [Faction::Cupric, Faction::Cupric] => "cc",
        }
    }

    fn from_scenario(scenario: &Scenario) -> Result<Self> {
        if scenario.players.len() != 2 {
            bail!(
                "neural-cup requires a 2-seat duel scenario, got {} seats",
                scenario.players.len()
            );
        }
        Ok(Self([
            scenario.players[0].faction,
            scenario.players[1].faction,
        ]))
    }

    fn faction(self, seat: u8) -> Faction {
        self.0[usize::from(seat)]
    }
}

impl std::str::FromStr for DuelFactions {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "ff" => Ok(Self([Faction::Ferrous, Faction::Ferrous])),
            "fc" => Ok(Self([Faction::Ferrous, Faction::Cupric])),
            "cf" => Ok(Self([Faction::Cupric, Faction::Ferrous])),
            "cc" => Ok(Self([Faction::Cupric, Faction::Cupric])),
            _ => Err(format!(
                "invalid faction pair {value:?}; expected ff, fc, cf, or cc"
            )),
        }
    }
}

/// Runtime profile for a native quantized neural cup.
#[derive(Debug, Clone, Copy)]
pub struct NeuralCupProfile {
    /// Decision cadence the network trained at.
    pub cadence: u64,
    /// Explicit hesitation rate per mille, or zero to derive it from skill.
    pub blunder: u32,
    /// Skill conditioning knob.
    pub skill: u32,
    /// Aggression conditioning knob.
    pub aggression: u32,
    /// Optional ordered roster override.
    pub factions: Option<DuelFactions>,
}

fn faction_name(faction: Faction) -> &'static str {
    match faction {
        Faction::Ferrous => "ferrous",
        Faction::Cupric => "cupric",
    }
}

fn prepare_cup_scenario(
    scenario: &str,
    seed: u64,
    factions: Option<DuelFactions>,
) -> Result<(Scenario, DuelFactions)> {
    let mut scenario = crate::runner::load_scenario(scenario)?;
    scenario.seed = seed;
    let authored = DuelFactions::from_scenario(&scenario)?;
    let actual = factions.unwrap_or(authored);
    if factions.is_some() {
        for seat in 0..2 {
            scenario.retint_seat(seat, actual.faction(seat as u8));
        }
    }
    Ok((scenario, actual))
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum Request {
    Reset {
        seed: u64,
        /// Externally-driven seats (default `[0]`).
        #[serde(default = "default_control")]
        control: Vec<u8>,
        /// Scripted opponent tier for any uncontrolled seat.
        #[serde(default = "default_tier")]
        tier: Difficulty,
        #[serde(default = "default_max_ticks")]
        max_ticks: u64,
        #[serde(default)]
        scenario: Option<String>,
        /// Optional roster for every scenario seat, in seat order.
        /// Omitted resets preserve the scenario's authored factions.
        #[serde(default)]
        factions: Option<Vec<Faction>>,
        /// Decision stride in ticks (default 8).
        #[serde(default = "default_cadence")]
        cadence: u64,
    },
    Step {
        /// One action per controlled seat, in `control` order.
        actions: Vec<usize>,
    },
    Quit,
}

fn default_control() -> Vec<u8> {
    vec![0]
}

fn default_tier() -> Difficulty {
    Difficulty::Veteran
}

fn default_max_ticks() -> u64 {
    40_000
}

fn default_cadence() -> u64 {
    8
}

struct Episode {
    state: State,
    gyms: Vec<GymBot>,
    opponents: Vec<Brain>,
    max_ticks: u64,
}

impl Episode {
    fn new(
        seed: u64,
        control: &[u8],
        tier: Difficulty,
        max_ticks: u64,
        scenario: Option<&str>,
        factions: Option<&[Faction]>,
        cadence: u64,
    ) -> Result<Self> {
        let mut scenario = crate::runner::load_scenario(scenario.unwrap_or("skirmish"))?;
        scenario.seed = seed;
        let players = scenario.players.len() as u8;
        if control.is_empty() || control.len() > players as usize {
            bail!("control must name 1..={players} seats");
        }
        let mut seen = control.to_vec();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() != control.len() || control.iter().any(|s| *s >= players) {
            bail!("controlled seats must be distinct and < {players}");
        }
        if let Some(factions) = factions {
            if factions.len() != usize::from(players) {
                bail!(
                    "factions must name exactly {players} seats, got {}",
                    factions.len()
                );
            }
            for (seat, &faction) in factions.iter().enumerate() {
                scenario.retint_seat(seat, faction);
            }
        }
        let state = scenario.build().context("scenario build")?;
        let gyms: Vec<GymBot> = control
            .iter()
            .map(|s| GymBot::with_cadence(PlayerId(*s), cadence))
            .collect();
        let opponents: Vec<Brain> = (0..players)
            .filter(|s| !control.contains(s))
            .map(|s| Brain::for_tier(PlayerId(s), seed, tier))
            .collect();
        Ok(Self {
            state,
            gyms,
            opponents,
            max_ticks,
        })
    }

    /// Whether a controlled seat still holds a Foundry. An eliminated
    /// learner's remnants keep fighting (sim rule), but its episode is
    /// over — spectating to the tick cap teaches nothing.
    fn seat_alive(&self, seat: PlayerId) -> bool {
        self.state
            .buildings()
            .iter()
            .any(|b| b.player == seat && b.kind == oxide_sim::BuildingKind::Foundry)
    }

    /// Whether any seat on this seat's *team* still holds a Foundry —
    /// the episode a dead learner belongs to lives while its team does.
    fn team_alive(&self, seat: PlayerId) -> bool {
        let team = self.state.player(seat).team;
        self.state.buildings().iter().any(|b| {
            b.kind == oxide_sim::BuildingKind::Foundry && self.state.player(b.player).team == team
        })
    }

    /// True while the match is live, any controlled seat's *team*
    /// stands, and the tick cap is unmet. A dead learner drops out of
    /// the per-frame `seats` list (its `alive` flag goes false) but its
    /// episode runs on to the team outcome — ending it at the learner's
    /// own death would score every self-sacrifice as a loss, even when
    /// the scripted ally goes on to win (the mixed-ally `team2` role
    /// fields exactly that shape). Per-seat trajectory truncation is
    /// the trainer's bookkeeping.
    fn live(&self) -> bool {
        self.state.result().is_none()
            && self.state.current_tick() < self.max_ticks
            && self.gyms.iter().any(|g| self.team_alive(g.player()))
    }

    fn cadence(&self) -> u64 {
        self.gyms[0].cadence()
    }

    /// Applies the trainer's actions at the current decision tick, then
    /// advances to the next decision tick (or the end).
    fn step(&mut self, actions: &[usize]) -> Result<()> {
        // One action per *living* controlled seat, in seat order — dead
        // learners dropped out of the frame's seats list and send none.
        let live: Vec<usize> = (0..self.gyms.len())
            .filter(|&i| self.seat_alive(self.gyms[i].player()))
            .collect();
        if actions.len() != live.len() {
            bail!(
                "expected {} actions (one per living controlled seat), got {}",
                live.len(),
                actions.len()
            );
        }
        let mut commands = Vec::new();
        for (&idx, action) in live.iter().zip(actions) {
            commands.extend(self.gyms[idx].step(&self.state, Action::from_index(*action)));
        }
        for op in self.opponents.iter_mut() {
            commands.extend(op.act(&self.state));
        }
        self.state.tick(&commands);
        while self.live() && !self.state.current_tick().is_multiple_of(self.cadence()) {
            let mut commands = Vec::new();
            for op in self.opponents.iter_mut() {
                commands.extend(op.act(&self.state));
            }
            self.state.tick(&commands);
        }
        Ok(())
    }

    fn reply(&mut self) -> serde_json::Value {
        let alive: Vec<u8> = self
            .gyms
            .iter()
            .map(|g| g.player())
            .filter(|p| self.seat_alive(*p))
            .map(|p| p.0)
            .collect();
        let factions: Vec<Faction> = self.state.players().iter().map(|p| p.faction).collect();
        if self.live() {
            let state = &self.state;
            let live_seats: Vec<PlayerId> = self
                .gyms
                .iter()
                .map(|g| g.player())
                .filter(|p| {
                    state
                        .buildings()
                        .iter()
                        .any(|b| b.player == *p && b.kind == oxide_sim::BuildingKind::Foundry)
                })
                .collect();
            let seats: Vec<_> = self
                .gyms
                .iter_mut()
                .filter(|g| live_seats.contains(&g.player()))
                .map(|gym| {
                    let d = gym.decision(state);
                    serde_json::json!({
                        "seat": gym.player().0,
                        "features": d.features.to_vec(),
                        "mask": d.mask.to_vec(),
                    })
                })
                .collect();
            serde_json::json!({
                "done": false,
                "tick": self.state.current_tick(),
                "seats": seats,
                "alive": alive,
                "factions": factions,
            })
        } else {
            // `winner` is the surviving *team*; in a 1v1 (where every
            // seat defaults to its own team) that is also the seat index.
            let winner = match self.state.result() {
                Some(GameResult::Victory { team }) => Some(team),
                _ => None,
            };
            let winners: Vec<u8> = self.state.winners().into_iter().map(|p| p.0).collect();
            // v5: the terminal frame carries observations for LIVING
            // controlled seats, matching the nonterminal contract — a
            // post-mortem row for a dead seat would zero the frozen
            // last view its padding exists to preserve. The trainer
            // prices terminal potential and the tech bonus off this
            // frame instead of the previous decision's stale one.
            let state = &self.state;
            let live_seats: Vec<PlayerId> = self
                .gyms
                .iter()
                .map(|g| g.player())
                .filter(|p| {
                    state
                        .buildings()
                        .iter()
                        .any(|b| b.player == *p && b.kind == oxide_sim::BuildingKind::Foundry)
                })
                .collect();
            let seats: Vec<_> = self
                .gyms
                .iter_mut()
                .filter(|g| live_seats.contains(&g.player()))
                .map(|gym| {
                    let d = gym.decision(state);
                    serde_json::json!({
                        "seat": gym.player().0,
                        "features": d.features.to_vec(),
                        "mask": d.mask.to_vec(),
                    })
                })
                .collect();
            serde_json::json!({
                "done": true,
                "tick": self.state.current_tick(),
                "winner": winner,
                "winners": winners,
                "alive": alive,
                "seats": seats,
                "factions": factions,
            })
        }
    }
}

/// Runs the stdio loop until EOF or a quit command.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "{}",
        serde_json::json!({
            "ready": true,
            "version": GYM_VERSION,
            "names": oxide_sim::bot::FEATURE_NAMES.to_vec(),
            "features": FEATURE_COUNT,
            "actions": ACTION_COUNT,
            "reset_factions": true,
        })
    )?;
    out.flush()?;

    let mut episode: Option<Episode> = None;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Reset {
                seed,
                control,
                tier,
                max_ticks,
                scenario,
                factions,
                cadence,
            }) => match Episode::new(
                seed,
                &control,
                tier,
                max_ticks,
                scenario.as_deref(),
                factions.as_deref(),
                cadence,
            ) {
                Ok(mut e) => {
                    let reply = e.reply();
                    episode = Some(e);
                    reply
                }
                Err(err) => serde_json::json!({ "error": format!("{err:#}") }),
            },
            Ok(Request::Step { actions }) => match episode.as_mut() {
                Some(e) if e.live() => match e.step(&actions) {
                    Ok(()) => e.reply(),
                    Err(err) => serde_json::json!({ "error": format!("{err:#}") }),
                },
                Some(_) => serde_json::json!({ "error": "episode is over; reset first" }),
                None => serde_json::json!({ "error": "no episode; reset first" }),
            },
            Ok(Request::Quit) => break,
            Err(err) => serde_json::json!({ "error": format!("bad request: {err}") }),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

/// Runs the promotion tournament for a quantized artifact: every
/// scripted tier, `seeds` seeds x both seats, printed as JSON lines.
/// This measures the shipped integer bot — the float checkpoint it
/// came from is a different (unshippable) player.
pub fn neural_cup(
    weights: &std::path::Path,
    seeds: u64,
    scenario: &str,
    profile: NeuralCupProfile,
) -> Result<()> {
    use oxide_sim::bot::{NeuralBot, QuantNet};
    let NeuralCupProfile {
        cadence,
        blunder,
        skill,
        aggression,
        factions,
    } = profile;
    let json = std::fs::read_to_string(weights)
        .with_context(|| format!("reading {}", weights.display()))?;
    let net = QuantNet::from_json(&json).map_err(|e| anyhow::anyhow!(e))?;
    // Every result line carries the artifact's digest: a cup table
    // pasted into an experiments note answers "which weights" on its
    // own, long after the checkpoint path stops meaning anything.
    let digest = format!("{:016x}", net.digest());
    let (_, actual_factions) = prepare_cup_scenario(scenario, 3000, factions)?;
    let faction_source = if factions.is_some() {
        "override"
    } else {
        "authored"
    };
    eprintln!(
        "artifact: {} · digest {digest} · factions {} ({faction_source})",
        weights.display(),
        actual_factions.code()
    );
    for tier in [
        Difficulty::Scrapheap,
        Difficulty::Standard,
        Difficulty::Veteran,
        Difficulty::Prime,
    ] {
        // Every (seed, seat) game is an independent deterministic sim, so
        // they run across threads; aggregation folds a pre-ordered result
        // vector, keeping the printed numbers identical to the serial
        // loop whatever the scheduling.
        let pairs: Vec<(u64, u8)> = (3000..3000 + seeds)
            .flat_map(|seed| [(seed, 0u8), (seed, 1u8)])
            .collect();
        let play = |&(seed, seat): &(u64, u8)| -> Result<(bool, bool, u64)> {
            let (sc, game_factions) = prepare_cup_scenario(scenario, seed, factions)?;
            debug_assert_eq!(game_factions, actual_factions);
            let mut state = sc.build().context("scenario build")?;
            let faction = game_factions.faction(seat);
            let mut neural = NeuralBot::with_profile(
                PlayerId(seat),
                cadence,
                net.clone(),
                skill,
                aggression,
                faction,
                blunder,
                seed,
            );
            let mut opponent = Brain::for_tier(PlayerId(1 - seat), seed, tier);
            for _ in 0..40_000u32 {
                let mut commands = neural.act(&state);
                commands.extend(opponent.act(&state));
                state.tick(&commands);
                if state.result().is_some() {
                    break;
                }
            }
            // Score by seat membership, not team id — a team number
            // only coincides with the seat index on default-team maps.
            let won = matches!(state.result(), Some(GameResult::Victory { .. }))
                && state.winners().contains(&PlayerId(seat));
            let draw = !matches!(state.result(), Some(GameResult::Victory { .. }));
            Ok((won, draw, state.current_tick()))
        };
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(pairs.len().max(1));
        let chunk = pairs.len().div_ceil(threads);
        let mut outcomes: Vec<Result<(bool, bool, u64)>> = Vec::with_capacity(pairs.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = pairs
                .chunks(chunk)
                .map(|slice| scope.spawn(move || slice.iter().map(play).collect::<Vec<_>>()))
                .collect();
            for handle in handles {
                outcomes.extend(handle.join().expect("cup game thread panicked"));
            }
        });
        let (mut wins, mut draws, mut ticks) = (0u64, 0u64, Vec::new());
        let mut seat_wins = [0u64; 2];
        let mut seat_draws = [0u64; 2];
        let mut seat_ticks = [Vec::new(), Vec::new()];
        for ((_, seat), outcome) in pairs.iter().copied().zip(outcomes) {
            let (won, draw, tick) = outcome?;
            wins += u64::from(won);
            draws += u64::from(draw);
            ticks.push(tick);
            seat_wins[usize::from(seat)] += u64::from(won);
            seat_draws[usize::from(seat)] += u64::from(draw);
            seat_ticks[usize::from(seat)].push(tick);
        }
        ticks.sort_unstable();
        for values in &mut seat_ticks {
            values.sort_unstable();
        }
        let games = seeds * 2;
        let by_seat: Vec<_> = (0..2)
            .map(|seat| {
                let seat = seat as u8;
                let values = &seat_ticks[usize::from(seat)];
                serde_json::json!({
                    "seat": seat,
                    "faction": faction_name(actual_factions.faction(seat)),
                    "wins": seat_wins[usize::from(seat)],
                    "draws": seat_draws[usize::from(seat)],
                    "games": seeds,
                    "median_ticks": values[values.len() / 2],
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "opponent": format!("{tier:?}"),
                "digest": digest,
                "factions": actual_factions.code(),
                "factions_source": faction_source,
                "wins": wins,
                "draws": draws,
                "games": games,
                "median_ticks": ticks[ticks.len() / 2],
                "by_seat": by_seat,
            })
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(factions: Option<&[Faction]>) -> Episode {
        Episode::new(17, &[0, 1], Difficulty::Veteran, 100, None, factions, 8)
            .expect("skirmish episode")
    }

    #[test]
    fn every_duel_faction_pair_parses_canonically() {
        for (text, expected) in [
            ("ff", [Faction::Ferrous, Faction::Ferrous]),
            ("FC", [Faction::Ferrous, Faction::Cupric]),
            ("cf", [Faction::Cupric, Faction::Ferrous]),
            ("cc", [Faction::Cupric, Faction::Cupric]),
        ] {
            let pair = text.parse::<DuelFactions>().expect("valid pair");
            assert_eq!(pair.0, expected);
            assert_eq!(pair.code(), text.to_ascii_lowercase());
        }
    }

    #[test]
    fn invalid_duel_faction_pairs_are_rejected() {
        for text in ["", "f", "fcc", "fx", "ferrous-cupric"] {
            let err = text
                .parse::<DuelFactions>()
                .expect_err("invalid faction pair");
            assert!(
                err.contains("expected ff, fc, cf, or cc"),
                "unexpected error for {text:?}: {err}"
            );
        }
    }

    #[test]
    fn cup_overrides_cover_both_factions_from_both_physical_seats() {
        let mut seen = [[false; 2]; 2];
        for text in ["ff", "fc", "cf", "cc"] {
            let requested = text.parse::<DuelFactions>().expect("valid pair");
            let (scenario, actual) = prepare_cup_scenario("skirmish", 41, Some(requested))
                .expect("retinted cup scenario");
            assert_eq!(actual, requested);
            for (seat, seen_factions) in seen.iter_mut().enumerate() {
                let faction = scenario.players[seat].faction;
                assert_eq!(faction, requested.0[seat]);
                seen_factions[usize::from(faction == Faction::Cupric)] = true;
            }
        }
        assert_eq!(seen, [[true, true], [true, true]]);
    }

    #[test]
    fn omitted_cup_factions_preserve_the_scenario_exactly() {
        let mut authored = Scenario::skirmish();
        authored.seed = 41;
        let (prepared, actual) =
            prepare_cup_scenario("skirmish", 41, None).expect("authored cup scenario");
        assert_eq!(prepared, authored);
        assert_eq!(actual.code(), "fc");
    }

    #[test]
    fn cup_rejects_non_duel_scenarios_before_building() {
        let mut scenario = Scenario::skirmish();
        scenario.players.push(scenario.players[0].clone());
        let err = DuelFactions::from_scenario(&scenario).expect_err("three-seat cup");
        assert!(
            err.to_string()
                .contains("neural-cup requires a 2-seat duel scenario, got 3 seats"),
            "{err:#}"
        );
    }

    #[test]
    fn reset_factions_are_optional_in_the_wire_contract() {
        let old = serde_json::from_str::<Request>(
            r#"{"cmd":"reset","seed":1,"control":[0,1],"max_ticks":100}"#,
        )
        .expect("old reset request");
        match old {
            Request::Reset { factions, .. } => assert!(factions.is_none()),
            _ => panic!("parsed the wrong request"),
        }

        let new = serde_json::from_str::<Request>(
            r#"{"cmd":"reset","seed":1,"control":[0,1],"factions":["cupric","ferrous"]}"#,
        )
        .expect("retinted reset request");
        match new {
            Request::Reset { factions, .. } => {
                assert_eq!(factions, Some(vec![Faction::Cupric, Faction::Ferrous]))
            }
            _ => panic!("parsed the wrong request"),
        }
    }

    #[test]
    fn every_two_seat_faction_pair_reaches_state_and_observation() {
        let faction_feature = oxide_sim::bot::FEATURE_NAMES
            .iter()
            .position(|name| *name == "faction")
            .expect("gym contract carries faction");
        for factions in [
            [Faction::Ferrous, Faction::Ferrous],
            [Faction::Ferrous, Faction::Cupric],
            [Faction::Cupric, Faction::Ferrous],
            [Faction::Cupric, Faction::Cupric],
        ] {
            let mut episode = episode(Some(&factions));
            assert_eq!(episode.state.player(PlayerId(0)).faction, factions[0]);
            assert_eq!(episode.state.player(PlayerId(1)).faction, factions[1]);

            let reply = episode.reply();
            assert_eq!(
                reply["factions"],
                serde_json::json!([factions[0], factions[1]])
            );
            let seats = reply["seats"].as_array().expect("seat observations");
            for (seat, faction) in factions.into_iter().enumerate() {
                let view = seats
                    .iter()
                    .find(|view| view["seat"] == seat as u8)
                    .expect("controlled seat");
                assert_eq!(
                    view["features"][faction_feature],
                    i64::from(faction == Faction::Cupric),
                    "seat {seat} observation must report its retinted faction"
                );
            }
        }
    }

    #[test]
    fn omitted_factions_preserve_the_authored_rosters() {
        let episode = episode(None);
        assert_eq!(episode.state.player(PlayerId(0)).faction, Faction::Ferrous);
        assert_eq!(episode.state.player(PlayerId(1)).faction, Faction::Cupric);
    }

    #[test]
    fn a_partial_faction_list_is_rejected_instead_of_misapplied() {
        let err = Episode::new(
            17,
            &[0],
            Difficulty::Veteran,
            100,
            None,
            Some(&[Faction::Cupric]),
            8,
        )
        .err()
        .expect("one faction cannot describe a two-seat scenario");
        assert!(
            err.to_string()
                .contains("factions must name exactly 2 seats, got 1"),
            "{err:#}"
        );
    }
}
