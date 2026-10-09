//! Turning a filled-in draft into a match: the scenario it describes, the
//! bot identities it gets, and whether it starts locally or as a hosted lobby.

use super::NewMatchDraft;
use crate::game::Game;
use crate::numeric::Fit;
use anyhow::{Context, Result};
use chassis::rng::Pcg32;
use oxide_sim::Scenario;
use std::time::{SystemTime, UNIX_EPOCH};

const BOT_PERSONALITY_STREAM: u64 = 0x0B07_5EED;
pub(crate) const BOT_PERSONALITY_WINDOW: u64 = 16;
const AUTOMATION_PERSONALITY_SEED: u64 = 0xA117_0A7E_0B07_5EED;

#[derive(Debug, Clone)]
pub(crate) struct PersonalitySeedSource {
    next_base: u64,
}

impl PersonalitySeedSource {
    pub(crate) fn for_session(automation: bool) -> Self {
        Self::for_session_with_entropy(automation, Self::from_entropy)
    }

    pub(crate) fn for_session_with_entropy(
        automation: bool,
        entropy: impl FnOnce() -> PersonalitySeedSource,
    ) -> Self {
        if automation {
            Self::from_seed(AUTOMATION_PERSONALITY_SEED)
        } else {
            entropy()
        }
    }

    pub(crate) fn from_entropy() -> Self {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos(),
            Err(error) => error.duration().as_nanos(),
        };
        let folded_time = (nanos & 0xFFFF_FFFF_FFFF_FFFF) as u64 ^ (nanos >> 64) as u64;
        let process = u64::from(std::process::id()).rotate_left(32);
        Self::from_seed(folded_time ^ process)
    }

    pub(crate) fn from_seed(seed: u64) -> Self {
        let mut rng = Pcg32::new(seed, BOT_PERSONALITY_STREAM);
        Self {
            // Scenario validation caps a roster at 16. Alignment makes
            // `base + seat` a complete, non-wrapping 16-seed window.
            next_base: rng.next_u64() & !(BOT_PERSONALITY_WINDOW - 1),
        }
    }

    pub(crate) fn match_base(&self) -> u64 {
        self.next_base
    }

    pub(crate) fn commit_launch(&mut self) {
        self.next_base = self.next_base.wrapping_add(BOT_PERSONALITY_WINDOW);
    }
}

/// Builds the game a filled-in draft describes.
pub(crate) fn launch(draft: &NewMatchDraft, personality_seed_base: u64) -> Result<Game> {
    Game::new(draft_scenario(draft, personality_seed_base)?)
}

/// What starting a draft built.
pub(crate) enum NewMatch {
    /// A match on this machine alone.
    Local(Box<Game>),
    /// A lobby hosting the match for its remote chairs.
    Hosted(Box<crate::netplay::HostLobby>),
}

/// Starts a draft: a local match, or a lobby listening on `bind` when a
/// chair is remote.
pub(crate) fn start_new_match(
    draft: &NewMatchDraft,
    personality_seed_base: u64,
    bind: &str,
) -> Result<NewMatch> {
    if !super::draft_hosts(draft) {
        return Ok(NewMatch::Local(Box::new(launch(
            draft,
            personality_seed_base,
        )?)));
    }
    let scenario = draft_scenario(draft, personality_seed_base)?;
    let host = oxide_sim::PlayerId(
        draft
            .seat_choice
            .min(scenario.players.len() - 1)
            .fit::<u8>(),
    );
    let lobby =
        crate::netplay::HostLobby::new(bind, scenario, host, &crate::build_identity().revision)?;
    Ok(NewMatch::Hosted(Box::new(lobby)))
}

/// The scenario a filled-in draft describes.
pub(crate) fn draft_scenario(
    draft: &NewMatchDraft,
    personality_seed_base: u64,
) -> Result<Scenario> {
    let mut scenario = (**draft.scenario.as_ref().context("draft has a map")?).clone();
    // Seats come from the per-seat plans the setup screen filled.
    // Every opponent runs with its chosen difficulty and stance. This launch base gives each chair a distinct
    // hidden identity; after this point it is ordinary scenario/replay data.
    anyhow::ensure!(
        draft.seats.len() == scenario.players.len(),
        "draft seats out of step with the map"
    );
    // Discovery lists every parseable JSON without building it, so a
    // zero-seat file can reach here — refuse it as a launch error
    // instead of underflowing the seat clamp below.
    anyhow::ensure!(!scenario.players.is_empty(), "the map has no player seats");
    let seat_choice = draft.seat_choice.min(scenario.players.len() - 1);
    for (i, player) in scenario.players.iter_mut().enumerate() {
        let plan = draft.seats[i];
        player.bot = i != seat_choice && !plan.remote;
        player.bot_config = player.bot.then(|| {
            oxide_sim::scenario::BotConfig::new(
                plan.difficulty,
                plan.stance,
                personality_seed_base.wrapping_add(i as u64),
            )
        });
    }
    // Per-seat faction chips: Auto keeps the authored roster; an override
    // retints only that seat, starting units remapped through their roles.
    // Same-faction opponents stay readable because allegiance accents
    // carry friend-or-foe.
    for (i, plan) in draft.seats.iter().enumerate() {
        if let Some(faction) = super::faction_override(plan.faction_choice) {
            scenario.retint_seat(i, faction);
        }
    }
    // Per-seat team chips regroup seats without touching factions: an FFA
    // chip drops the seat onto its own team, and the sim densifies chosen
    // ids by first appearance at build. The scenario carries the choice,
    // so saves and replays reproduce the grouping. An all-one-team draft
    // fails the build (OneTeam) like any other launch error; the wizard
    // refuses it earlier with the reason inline.
    for (i, plan) in draft.seats.iter().enumerate() {
        scenario.players[i].team = super::team_override(plan.team_choice);
    }
    // Seat names must stay unique: the victory banner, the panel, and
    // the stats screen all address seats by name. Retints can land two
    // seats on one faction-derived label ("North West Ferrous" twice),
    // so duplicates take an ordinal instead of refusing to launch.
    let mut seen: Vec<String> = Vec::new();
    for player in &mut scenario.players {
        if seen.contains(&player.name) {
            let mut n = 2;
            while seen.contains(&format!("{} {n}", player.name)) {
                n += 1;
            }
            player.name = format!("{} {n}", player.name);
        }
        seen.push(player.name.clone());
    }
    let mut names: Vec<&str> = scenario.players.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    anyhow::ensure!(
        names.len() == scenario.players.len(),
        "seat names collide after setup"
    );
    Ok(scenario)
}

#[cfg(test)]
mod tests;
