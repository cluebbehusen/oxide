//! Construction-time bot profiles.
//!
//! Named styles, their curated variants, and complementary team roles
//! resolve once when a scenario seats its bots. Every draw owns a PCG
//! stream disjoint from the neural ladder's hesitation stream, so adding
//! profile variety cannot advance or reseed execution mistakes.

use super::neural::{
    CONDITIONING_COUNT, Level, ladder_condition_values, ladder_condition_values_with_facets,
};
use crate::ids::PlayerId;
use crate::map::{Map, MapError};
use crate::scenario::{BotConfig, BotConfigError, NamedStyle, Scenario, TeamRole};
use crate::state::Faction;
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;

/// Number of curated variants within every named style.
pub const NAMED_VARIANT_COUNT: u8 = 3;

// Strong facets cross this shared boundary into GymBot's finite authored
// doctrine instead of remaining only a neural preference.
pub(super) const PROFILE_DOCTRINE_THRESHOLD: u32 = 800;
/// Only an explicit Vanguard role receives the finite direct-ground screen.
/// Aggressive solo profiles peak below this threshold, preserving their
/// learned openings instead of silently rewriting every high-commitment game.
pub(super) const PROFILE_COMMITMENT_THRESHOLD: u32 = 1000;

/// Number of high-level profile facets appended to the neural condition.
pub const PROFILE_CONDITION_COUNT: usize = 5;

/// Profile facet names in gym-conditioning order.
pub const PROFILE_CONDITION_NAMES: [&str; PROFILE_CONDITION_COUNT] = [
    "profile_economy",
    "profile_air",
    "profile_siege",
    "profile_support",
    "profile_commitment",
];

/// First stream selector used to deal a named style, plus the seat id.
pub const PROFILE_STYLE_STREAM_BASE: u64 = 5000;
/// First stream selector used to deal a style variant, plus the seat id.
pub const PROFILE_VARIANT_STREAM_BASE: u64 = 6000;
/// Scenario-wide stream selector used to permute complementary team jobs.
pub const PROFILE_ROLE_STREAM: u64 = 7000;
/// Scenario-wide stream used to shuffle the least-used named-style deck.
const PROFILE_STYLE_DECK_STREAM: u64 = 7100;
/// First scenario-wide stream used to shuffle each style's variant deck.
const PROFILE_VARIANT_DECK_STREAM_BASE: u64 = 7200;

/// High-level strategy inputs in the policy conditioning vector.
///
/// These values resolve once so setup, diagnostics, runtime inference, and
/// training all share one contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileFacets {
    /// Preference for compounding economy.
    pub economy_bias: u32,
    /// Preference for air production and raids.
    pub air_bias: u32,
    /// Preference for artillery and siege infrastructure.
    pub siege_bias: u32,
    /// Preference for protection, repair, and allied coverage.
    pub support_bias: u32,
    /// Willingness to commit armies and sustain pressure.
    pub commitment_bias: u32,
}

impl ProfileFacets {
    /// No named strategic lean.
    ///
    /// Raw-aggression experiments use this value so widening the actor
    /// does not silently invent a profile they did not request.
    pub const ZERO: Self = Self {
        economy_bias: 0,
        air_bias: 0,
        siege_bias: 0,
        support_bias: 0,
        commitment_bias: 0,
    };

    /// Reconstructs facets from the published profile-condition order.
    ///
    /// Callers accepting untrusted values remain responsible for enforcing
    /// the documented `0..=1000` range.
    pub const fn from_conditions(conditions: [u32; PROFILE_CONDITION_COUNT]) -> Self {
        Self {
            economy_bias: conditions[0],
            air_bias: conditions[1],
            siege_bias: conditions[2],
            support_bias: conditions[3],
            commitment_bias: conditions[4],
        }
    }

    /// Values aligned with [`PROFILE_CONDITION_NAMES`], each in `0..=1000`.
    pub const fn conditions(self) -> [u32; PROFILE_CONDITION_COUNT] {
        [
            self.economy_bias,
            self.air_bias,
            self.siege_bias,
            self.support_bias,
            self.commitment_bias,
        ]
    }

    /// Applies one complementary team job to the authored base profile.
    pub fn with_role(self, role: TeamRole) -> Self {
        let adjust = |value: u32, delta: i32| {
            if delta >= 0 {
                value.saturating_add(delta as u32).min(1000)
            } else {
                value.saturating_sub(delta.unsigned_abs())
            }
        };
        match role {
            TeamRole::Generalist => self,
            TeamRole::Vanguard => Self {
                support_bias: adjust(self.support_bias, -50),
                commitment_bias: adjust(self.commitment_bias, 150)
                    .max(PROFILE_COMMITMENT_THRESHOLD),
                ..self
            },
            TeamRole::Industry => Self {
                economy_bias: adjust(self.economy_bias, 200),
                commitment_bias: adjust(self.commitment_bias, -50),
                ..self
            },
            TeamRole::Support => Self {
                economy_bias: adjust(self.economy_bias, 50),
                support_bias: adjust(self.support_bias, 200),
                ..self
            },
            TeamRole::Siege => Self {
                air_bias: adjust(self.air_bias, -50),
                siege_bias: adjust(self.siege_bias, 200).max(PROFILE_DOCTRINE_THRESHOLD),
                ..self
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct VariantSpec {
    name: &'static str,
    aggression: u32,
    facets: ProfileFacets,
}

/// One authored named-style variant before a team-role adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalProfile {
    /// Setup-visible style family.
    pub style: NamedStyle,
    /// Stable zero-based variant within the style.
    pub variant: u8,
    /// Human-readable diagnostic key.
    pub name: &'static str,
    /// Legacy aggression condition retained by the widened policy.
    pub aggression: u32,
    /// Authored generalist facets before a team-role adjustment.
    pub facets: ProfileFacets,
}

/// Every team role represented in the canonical gym catalog.
pub const PROFILE_TEAM_ROLES: [TeamRole; 5] = [
    TeamRole::Generalist,
    TeamRole::Vanguard,
    TeamRole::Industry,
    TeamRole::Support,
    TeamRole::Siege,
];

/// Returns every named style variant in stable setup order.
pub fn canonical_profiles() -> Vec<CanonicalProfile> {
    NamedStyle::ALL
        .into_iter()
        .flat_map(|style| {
            (0..NAMED_VARIANT_COUNT).map(move |variant| {
                let spec = variant_spec(style, variant);
                CanonicalProfile {
                    style,
                    variant,
                    name: spec.name,
                    aggression: spec.aggression,
                    facets: spec.facets,
                }
            })
        })
        .collect()
}

fn variant_spec(style: NamedStyle, variant: u8) -> VariantSpec {
    let variants = match style {
        NamedStyle::Turtle => [
            VariantSpec {
                name: "fortress",
                aggression: 100,
                facets: ProfileFacets {
                    economy_bias: 600,
                    air_bias: 250,
                    siege_bias: 450,
                    support_bias: 850,
                    commitment_bias: 200,
                },
            },
            VariantSpec {
                name: "industrial-attrition",
                aggression: 170,
                facets: ProfileFacets {
                    economy_bias: 900,
                    air_bias: 200,
                    siege_bias: 400,
                    support_bias: 650,
                    commitment_bias: 300,
                },
            },
            VariantSpec {
                name: "counterbattery",
                aggression: 240,
                facets: ProfileFacets {
                    economy_bias: 550,
                    air_bias: 150,
                    siege_bias: 850,
                    support_bias: 700,
                    commitment_bias: 350,
                },
            },
        ],
        NamedStyle::Balanced => [
            VariantSpec {
                name: "ground-combined",
                aggression: 500,
                facets: ProfileFacets {
                    economy_bias: 550,
                    air_bias: 300,
                    siege_bias: 400,
                    support_bias: 550,
                    commitment_bias: 500,
                },
            },
            VariantSpec {
                name: "air-combined",
                aggression: 550,
                facets: ProfileFacets {
                    economy_bias: 500,
                    air_bias: 850,
                    siege_bias: 250,
                    support_bias: 450,
                    commitment_bias: 550,
                },
            },
            VariantSpec {
                name: "siege-combined",
                aggression: 600,
                facets: ProfileFacets {
                    economy_bias: 500,
                    air_bias: 200,
                    siege_bias: 850,
                    support_bias: 500,
                    commitment_bias: 600,
                },
            },
        ],
        NamedStyle::Aggressive => [
            VariantSpec {
                name: "swarm",
                aggression: 760,
                facets: ProfileFacets {
                    economy_bias: 300,
                    air_bias: 350,
                    siege_bias: 250,
                    support_bias: 250,
                    commitment_bias: 850,
                },
            },
            VariantSpec {
                name: "air-raider",
                aggression: 850,
                facets: ProfileFacets {
                    economy_bias: 250,
                    air_bias: 900,
                    siege_bias: 150,
                    support_bias: 200,
                    commitment_bias: 900,
                },
            },
            VariantSpec {
                name: "siege-breaker",
                aggression: 940,
                facets: ProfileFacets {
                    economy_bias: 250,
                    air_bias: 150,
                    siege_bias: 900,
                    support_bias: 200,
                    commitment_bias: 950,
                },
            },
        ],
    };
    variants[usize::from(variant)]
}

/// A bot's fully resolved construction-time profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBotProfile {
    /// Execution rung on the player-facing difficulty ladder.
    pub level: Level,
    /// Named style, or `None` for an exact legacy aggression selection.
    pub style: Option<NamedStyle>,
    /// Curated named-style variant, or `None` for exact legacy aggression.
    pub variant: Option<u8>,
    /// Legacy aggression condition retained in the widened neural input.
    pub aggression: u32,
    /// Complementary team job.
    pub team_role: TeamRole,
    /// Reserved high-level strategy facets, including the team-role lean.
    pub facets: ProfileFacets,
}

impl ResolvedBotProfile {
    /// Human-readable curated variant key, absent for raw aggression.
    pub fn variant_name(self) -> Option<&'static str> {
        Some(variant_spec(self.style?, self.variant?).name)
    }

    /// The complete policy-conditioning values.
    pub fn conditions(self, faction: Faction) -> [i64; CONDITIONING_COUNT] {
        if self.style.is_some() {
            ladder_condition_values_with_facets(self.aggression, faction, self.facets)
        } else {
            ladder_condition_values(self.aggression, faction)
        }
    }
}

/// Why a scenario's construction-time bot profiles cannot be resolved.
#[derive(Debug, thiserror::Error)]
pub enum BotProfileError {
    /// The map text could not be interpreted for team-role symmetry.
    #[error(transparent)]
    Map(#[from] MapError),
    /// A seat's personality selection is internally inconsistent.
    #[error("player {player} has an invalid bot profile: {source}")]
    InvalidConfig {
        /// Seat carrying the invalid selection.
        player: PlayerId,
        /// The invalid combination.
        source: BotConfigError,
    },
    /// Mirrored seats authored different variants, which would bake in a side.
    #[error(
        "mirrored players {player} and {mirror} request conflicting variants ({variant} and {mirror_variant})"
    )]
    ConflictingMirrorVariants {
        /// First seat.
        player: PlayerId,
        /// Its 180-degree opponent.
        mirror: PlayerId,
        /// First requested variant.
        variant: u8,
        /// Opponent's requested variant.
        mirror_variant: u8,
    },
    /// Mirrored seats authored different jobs, which would bake in a side.
    #[error(
        "mirrored players {player} and {mirror} request conflicting team roles ({role:?} and {mirror_role:?})"
    )]
    ConflictingMirrorRoles {
        /// First seat.
        player: PlayerId,
        /// Its 180-degree opponent.
        mirror: PlayerId,
        /// First requested role.
        role: TeamRole,
        /// Opponent's requested role.
        mirror_role: TeamRole,
    },
}

/// Deals one of the three named styles from a seat-private stream.
pub fn deal_named_style(scenario_seed: u64, player: PlayerId) -> NamedStyle {
    let mut rng = Pcg32::new(
        scenario_seed,
        PROFILE_STYLE_STREAM_BASE + u64::from(player.0),
    );
    NamedStyle::ALL[rng.next_below(NamedStyle::ALL.len() as u32) as usize]
}

/// Deals one curated variant from a second seat-private stream.
pub fn deal_style_variant(scenario_seed: u64, player: PlayerId) -> u8 {
    let mut rng = Pcg32::new(
        scenario_seed,
        PROFILE_VARIANT_STREAM_BASE + u64::from(player.0),
    );
    rng.next_below(u32::from(NAMED_VARIANT_COUNT)) as u8
}

/// Resolves every configured bot in a scenario.
///
/// The returned vector stays aligned with `scenario.players`;
/// config-less seats are `None`.
pub fn resolve_bot_profiles(
    scenario: &Scenario,
) -> Result<Vec<Option<ResolvedBotProfile>>, BotProfileError> {
    let (map, anchors) = Map::parse(&scenario.map)?;
    resolve_bot_profiles_from_parts(scenario, &map, &anchors)
}

/// Resolves the team jobs for every seat, including the human chair.
pub fn resolve_team_roles(scenario: &Scenario) -> Result<Vec<TeamRole>, BotProfileError> {
    let (map, anchors) = Map::parse(&scenario.map)?;
    resolve_team_roles_from_parts(scenario, &map, &anchors)
}

pub(crate) fn resolve_bot_profiles_from_parts(
    scenario: &Scenario,
    map: &Map,
    anchors: &[(PlayerId, TilePos)],
) -> Result<Vec<Option<ResolvedBotProfile>>, BotProfileError> {
    for (index, player) in scenario.players.iter().enumerate() {
        if let Some(config) = player.bot_config {
            config
                .validate()
                .map_err(|source| BotProfileError::InvalidConfig {
                    player: PlayerId(index as u8),
                    source,
                })?;
        }
    }

    let roles = resolve_team_roles_from_parts(scenario, map, anchors)?;
    let mirrors = mirror_seats(scenario, map, anchors);
    let (dealt_styles, dealt_variants) = deal_named_profiles(scenario, &mirrors)?;
    Ok(scenario
        .players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            player.bot_config.map(|config| {
                resolve_one(
                    scenario.seed,
                    PlayerId(index as u8),
                    config,
                    roles[index],
                    dealt_styles[index],
                    dealt_variants[index],
                )
            })
        })
        .collect())
}

type DealtNamedProfiles = (Vec<Option<NamedStyle>>, Vec<Option<u8>>);

fn deal_named_profiles(
    scenario: &Scenario,
    mirrors: &[Option<usize>],
) -> Result<DealtNamedProfiles, BotProfileError> {
    let groups = named_deal_groups(scenario, mirrors);
    let mut group_styles: Vec<Option<NamedStyle>> = groups
        .iter()
        .map(|group| {
            group.iter().find_map(|seat| {
                scenario.players[*seat]
                    .bot_config
                    .and_then(|config| config.style)
            })
        })
        .collect();

    let mut style_counts = [0_u16; NamedStyle::ALL.len()];
    for style in group_styles.iter().flatten() {
        style_counts[style_index(*style)] += 1;
    }
    let style_preference = shuffled_styles(scenario.seed);
    for style in &mut group_styles {
        if style.is_none() {
            let dealt = least_used_style(&style_counts, &style_preference);
            style_counts[style_index(dealt)] += 1;
            *style = Some(dealt);
        }
    }

    let mut group_variants = vec![None; groups.len()];
    let mut variant_counts = [[0_u16; NAMED_VARIANT_COUNT as usize]; NamedStyle::ALL.len()];
    for (group_index, group) in groups.iter().enumerate() {
        let mut requested = None;
        for seat in group {
            let Some(variant) = scenario.players[*seat]
                .bot_config
                .and_then(|config| config.variant)
            else {
                continue;
            };
            if let Some(previous) = requested
                && previous != variant
            {
                return Err(BotProfileError::ConflictingMirrorVariants {
                    player: PlayerId(group[0] as u8),
                    mirror: PlayerId(*seat as u8),
                    variant: previous,
                    mirror_variant: variant,
                });
            }
            requested = Some(variant);
        }
        if let Some(variant) = requested {
            let style = group_styles[group_index].expect("every named group has a style");
            variant_counts[style_index(style)][usize::from(variant)] += 1;
            group_variants[group_index] = Some(variant);
        }
    }
    let variant_preferences = NamedStyle::ALL.map(|style| shuffled_variants(scenario.seed, style));
    for (group_index, variant) in group_variants.iter_mut().enumerate() {
        if variant.is_none() {
            let style = group_styles[group_index].expect("every named group has a style");
            let style_index = style_index(style);
            let dealt = least_used_variant(
                &variant_counts[style_index],
                &variant_preferences[style_index],
            );
            variant_counts[style_index][usize::from(dealt)] += 1;
            *variant = Some(dealt);
        }
    }

    let mut styles = vec![None; scenario.players.len()];
    let mut variants = vec![None; scenario.players.len()];
    for (group_index, group) in groups.iter().enumerate() {
        for seat in group {
            styles[*seat] = group_styles[group_index];
            variants[*seat] = group_variants[group_index];
        }
    }
    Ok((styles, variants))
}

fn named_deal_groups(scenario: &Scenario, mirrors: &[Option<usize>]) -> Vec<Vec<usize>> {
    let mut competitors = Vec::new();
    for index in 0..scenario.players.len() {
        let key = team_key(scenario, index);
        if !competitors.contains(&key) {
            competitors.push(key);
        }
    }
    let preserve_two_side_symmetry = competitors.len() == 2;
    let mut assigned = vec![false; scenario.players.len()];
    let mut groups = Vec::new();
    for seat in 0..scenario.players.len() {
        if assigned[seat] || !has_named_profile(scenario, seat) {
            continue;
        }
        let mirror = preserve_two_side_symmetry
            .then(|| mirrors[seat])
            .flatten()
            .filter(|mirror| {
                !assigned[*mirror]
                    && has_named_profile(scenario, *mirror)
                    && compatible_explicit_styles(scenario, seat, *mirror)
            });
        if let Some(mirror) = mirror {
            assigned[seat] = true;
            assigned[mirror] = true;
            groups.push(vec![seat, mirror]);
        } else {
            assigned[seat] = true;
            groups.push(vec![seat]);
        }
    }
    groups
}

fn has_named_profile(scenario: &Scenario, seat: usize) -> bool {
    scenario.players[seat]
        .bot_config
        .is_some_and(|config| config.aggression.is_none())
}

fn compatible_explicit_styles(scenario: &Scenario, a: usize, b: usize) -> bool {
    let a = scenario.players[a]
        .bot_config
        .and_then(|config| config.style);
    let b = scenario.players[b]
        .bot_config
        .and_then(|config| config.style);
    a.is_none() || b.is_none() || a == b
}

fn style_index(style: NamedStyle) -> usize {
    NamedStyle::ALL
        .iter()
        .position(|candidate| *candidate == style)
        .expect("named style belongs to the canonical palette")
}

fn shuffled_styles(seed: u64) -> [NamedStyle; NamedStyle::ALL.len()] {
    let mut styles = NamedStyle::ALL;
    let mut rng = Pcg32::new(seed, PROFILE_STYLE_DECK_STREAM);
    for upper in (1..styles.len()).rev() {
        let other = rng.next_below((upper + 1) as u32) as usize;
        styles.swap(upper, other);
    }
    styles
}

fn shuffled_variants(seed: u64, style: NamedStyle) -> [u8; NAMED_VARIANT_COUNT as usize] {
    let mut variants = [0, 1, 2];
    let mut rng = Pcg32::new(
        seed,
        PROFILE_VARIANT_DECK_STREAM_BASE + style_index(style) as u64,
    );
    for upper in (1..variants.len()).rev() {
        let other = rng.next_below((upper + 1) as u32) as usize;
        variants.swap(upper, other);
    }
    variants
}

fn least_used_style(
    counts: &[u16; NamedStyle::ALL.len()],
    preference: &[NamedStyle; NamedStyle::ALL.len()],
) -> NamedStyle {
    *preference
        .iter()
        .min_by_key(|style| counts[style_index(**style)])
        .expect("named style palette is non-empty")
}

fn least_used_variant(
    counts: &[u16; NAMED_VARIANT_COUNT as usize],
    preference: &[u8; NAMED_VARIANT_COUNT as usize],
) -> u8 {
    *preference
        .iter()
        .min_by_key(|variant| counts[usize::from(**variant)])
        .expect("variant palette is non-empty")
}

fn resolve_one(
    scenario_seed: u64,
    deal_player: PlayerId,
    config: BotConfig,
    team_role: TeamRole,
    dealt_style: Option<NamedStyle>,
    dealt_variant: Option<u8>,
) -> ResolvedBotProfile {
    if let Some(aggression) = config.aggression {
        return ResolvedBotProfile {
            level: config.level,
            style: None,
            variant: None,
            aggression,
            team_role,
            facets: ProfileFacets::ZERO,
        };
    }

    let style = dealt_style
        .or(config.style)
        .unwrap_or_else(|| deal_named_style(scenario_seed, deal_player));
    let variant = dealt_variant
        .or(config.variant)
        .unwrap_or_else(|| deal_style_variant(scenario_seed, deal_player));
    let spec = variant_spec(style, variant);
    ResolvedBotProfile {
        level: config.level,
        style: Some(style),
        variant: Some(variant),
        aggression: spec.aggression,
        team_role,
        facets: spec.facets.with_role(team_role),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeamKey {
    Authored(u8),
    Singleton(usize),
}

fn mirror_seats(
    scenario: &Scenario,
    map: &Map,
    anchors: &[(PlayerId, TilePos)],
) -> Vec<Option<usize>> {
    let (foundry_w, foundry_h) = BuildingKind::Foundry.base_stats().size;
    let mut candidates = vec![None; scenario.players.len()];
    for (player, anchor) in anchors {
        let image = TilePos::new(
            map.width() - foundry_w - anchor.x,
            map.height() - foundry_h - anchor.y,
        );
        let Some((mirror, _)) = anchors.iter().find(|(_, candidate)| *candidate == image) else {
            continue;
        };
        let player = usize::from(player.0);
        let mirror = usize::from(mirror.0);
        if player != mirror
            && mirror < scenario.players.len()
            && team_key(scenario, player) != team_key(scenario, mirror)
            && let Some(slot) = candidates.get_mut(player)
        {
            *slot = Some(mirror);
        }
    }
    (0..scenario.players.len())
        .map(|player| {
            let mirror = candidates[player]?;
            (candidates[mirror] == Some(player)).then_some(mirror)
        })
        .collect()
}

fn resolve_team_roles_from_parts(
    scenario: &Scenario,
    map: &Map,
    anchors: &[(PlayerId, TilePos)],
) -> Result<Vec<TeamRole>, BotProfileError> {
    let mut anchor_by_seat = vec![None; scenario.players.len()];
    for (player, anchor) in anchors {
        if let Some(slot) = anchor_by_seat.get_mut(usize::from(player.0)) {
            *slot = Some(*anchor);
        }
    }

    let (foundry_w, foundry_h) = BuildingKind::Foundry.base_stats().size;
    let pair_key = |index: usize| {
        let Some(anchor) = anchor_by_seat[index] else {
            return (i32::MAX, i32::MAX, index);
        };
        let image = TilePos::new(
            map.width() - foundry_w - anchor.x,
            map.height() - foundry_h - anchor.y,
        );
        let a = (anchor.y, anchor.x);
        let b = (image.y, image.x);
        let canonical = a.min(b);
        (canonical.0, canonical.1, index)
    };

    let mut teams: Vec<(TeamKey, Vec<usize>)> = Vec::new();
    for (index, player) in scenario.players.iter().enumerate() {
        let key = player
            .team
            .map_or(TeamKey::Singleton(index), TeamKey::Authored);
        match teams.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, seats)) => seats.push(index),
            None => teams.push((key, vec![index])),
        }
    }

    let mut palette = [
        TeamRole::Vanguard,
        TeamRole::Industry,
        TeamRole::Support,
        TeamRole::Siege,
    ];
    let mut role_rng = Pcg32::new(scenario.seed, PROFILE_ROLE_STREAM);
    for upper in (1..palette.len()).rev() {
        let other = role_rng.next_below((upper + 1) as u32) as usize;
        palette.swap(upper, other);
    }

    let mut roles = vec![TeamRole::Generalist; scenario.players.len()];
    for (_, mut seats) in teams {
        if seats.len() == 1 {
            continue;
        }
        seats.sort_by_key(|index| pair_key(*index));
        for (rank, seat) in seats.into_iter().enumerate() {
            roles[seat] = palette[rank % palette.len()];
        }
    }

    for (index, player) in scenario.players.iter().enumerate() {
        if let Some(role) = player.bot_config.and_then(|config| config.team_role) {
            roles[index] = role;
        }
    }

    for (index, mirror) in mirror_seats(scenario, map, anchors).into_iter().enumerate() {
        let Some(mirror) = mirror else {
            continue;
        };
        if index >= mirror {
            continue;
        }
        let requested = scenario.players[index]
            .bot_config
            .and_then(|config| config.team_role);
        let mirror_requested = scenario.players[mirror]
            .bot_config
            .and_then(|config| config.team_role);
        let role = match (requested, mirror_requested) {
            (Some(role), Some(mirror_role)) if role != mirror_role => {
                return Err(BotProfileError::ConflictingMirrorRoles {
                    player: PlayerId(index as u8),
                    mirror: PlayerId(mirror as u8),
                    role,
                    mirror_role,
                });
            }
            (Some(role), _) | (_, Some(role)) => role,
            (None, None) => roles[index],
        };
        roles[index] = role;
        roles[mirror] = role;
    }

    Ok(roles)
}

fn team_key(scenario: &Scenario, index: usize) -> TeamKey {
    scenario.players[index]
        .team
        .map_or(TeamKey::Singleton(index), TeamKey::Authored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_tables_are_complete_and_stay_inside_the_public_domain() {
        let mut names = Vec::new();
        for style in NamedStyle::ALL {
            let (min, max) = style.aggression_bounds();
            for variant in 0..NAMED_VARIANT_COUNT {
                let spec = variant_spec(style, variant);
                assert!((min..=max).contains(&spec.aggression));
                assert!(
                    spec.facets
                        .conditions()
                        .into_iter()
                        .all(|condition| condition <= 1000)
                );
                names.push(spec.name);
            }
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            usize::from(NAMED_VARIANT_COUNT) * NamedStyle::ALL.len(),
            "every curated commander has a distinct diagnostic name"
        );
    }

    #[test]
    fn style_variant_and_role_randomness_own_disjoint_stable_streams() {
        let first: Vec<_> = (0..8)
            .map(|seat| {
                (
                    deal_named_style(91, PlayerId(seat)),
                    deal_style_variant(91, PlayerId(seat)),
                )
            })
            .collect();
        let second: Vec<_> = (0..8)
            .map(|seat| {
                (
                    deal_named_style(91, PlayerId(seat)),
                    deal_style_variant(91, PlayerId(seat)),
                )
            })
            .collect();
        assert_eq!(first, second);
        assert_ne!(PROFILE_STYLE_STREAM_BASE, PROFILE_VARIANT_STREAM_BASE);
        assert_ne!(PROFILE_STYLE_STREAM_BASE, PROFILE_ROLE_STREAM);
        assert_ne!(PROFILE_VARIANT_STREAM_BASE, PROFILE_ROLE_STREAM);
    }

    #[test]
    fn vanguard_and_siege_roles_reach_finite_doctrine_without_losing_their_tradeoff() {
        let base = ProfileFacets {
            economy_bias: 500,
            air_bias: 300,
            siege_bias: 200,
            support_bias: 550,
            commitment_bias: 250,
        };
        let vanguard = base.with_role(TeamRole::Vanguard);
        assert_eq!(vanguard.commitment_bias, PROFILE_COMMITMENT_THRESHOLD);
        assert_eq!(vanguard.support_bias, 500);

        let siege = base.with_role(TeamRole::Siege);
        assert_eq!(siege.siege_bias, PROFILE_DOCTRINE_THRESHOLD);
        assert_eq!(siege.air_bias, 250);
    }
}
