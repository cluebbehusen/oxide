//! Construction-time bot profiles.
//!
//! Named styles, their curated variants, and complementary team roles
//! resolve once when a scenario seats its bots. Every draw owns a PCG
//! stream disjoint from the neural ladder's hesitation stream, so adding
//! profile variety cannot advance or reseed execution mistakes.

use super::neural::{CONDITIONING_COUNT, Level, ladder_condition_values};
use crate::ids::PlayerId;
use crate::map::{Map, MapError};
use crate::scenario::{BotConfig, BotConfigError, NamedStyle, Scenario, TeamRole};
use crate::state::Faction;
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;

/// Number of curated variants within every named style.
pub const NAMED_VARIANT_COUNT: u8 = 3;

/// Profile facet names in their future gym-conditioning order.
///
/// The values are resolved and inspectable in 0.14, but deliberately do
/// not widen the current v7 artifact's seven learned conditions.
pub const PROFILE_CONDITION_NAMES: [&str; 5] = [
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

/// High-level strategy inputs reserved for the gym-v8 widening.
///
/// These values resolve now so setup, diagnostics, and future training all
/// share one contract. The v7 network still receives only
/// [`ResolvedBotProfile::conditions`].
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
    /// Values aligned with [`PROFILE_CONDITION_NAMES`], each in `0..=1000`.
    pub const fn conditions(self) -> [u32; 5] {
        [
            self.economy_bias,
            self.air_bias,
            self.siege_bias,
            self.support_bias,
            self.commitment_bias,
        ]
    }

    fn with_role(self, role: TeamRole) -> Self {
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
                commitment_bias: adjust(self.commitment_bias, 150),
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
                siege_bias: adjust(self.siege_bias, 200),
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
    /// Execution rung on the shipped ladder.
    pub level: Level,
    /// Named style, or `None` for an exact legacy aggression selection.
    pub style: Option<NamedStyle>,
    /// Curated named-style variant, or `None` for exact legacy aggression.
    pub variant: Option<u8>,
    /// Exact v7 aggression condition supplied to the neural network.
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

    /// The current seven v7 conditioning values.
    ///
    /// Profile facets deliberately do not ride here yet. Gym-v8 will widen
    /// the learned contract and artifact together instead of silently
    /// feeding a v7 policy extra columns.
    pub fn conditions(self, faction: Faction) -> [i64; CONDITIONING_COUNT] {
        ladder_condition_values(self.aggression, faction)
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
/// The returned vector stays aligned with `scenario.players`; classic
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
    let mut mirrored_variants = vec![None; scenario.players.len()];
    for (index, mirror) in mirrors.iter().enumerate() {
        let Some(mirror) = *mirror else {
            continue;
        };
        if index >= mirror {
            continue;
        }
        let requested = scenario.players[index]
            .bot_config
            .and_then(|config| config.variant);
        let mirror_requested = scenario.players[mirror]
            .bot_config
            .and_then(|config| config.variant);
        let variant = match (requested, mirror_requested) {
            (Some(variant), Some(mirror_variant)) if variant != mirror_variant => {
                return Err(BotProfileError::ConflictingMirrorVariants {
                    player: PlayerId(index as u8),
                    mirror: PlayerId(mirror as u8),
                    variant,
                    mirror_variant,
                });
            }
            (Some(variant), _) | (_, Some(variant)) => Some(variant),
            (None, None) => None,
        };
        mirrored_variants[index] = variant;
        mirrored_variants[mirror] = variant;
    }
    Ok(scenario
        .players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            player.bot_config.map(|config| {
                let deal_seat = mirrors[index].map_or(index, |mirror| index.min(mirror));
                resolve_one(
                    scenario.seed,
                    PlayerId(deal_seat as u8),
                    config,
                    roles[index],
                    mirrored_variants[index],
                )
            })
        })
        .collect())
}

fn resolve_one(
    scenario_seed: u64,
    deal_player: PlayerId,
    config: BotConfig,
    team_role: TeamRole,
    mirrored_variant: Option<u8>,
) -> ResolvedBotProfile {
    if let Some(aggression) = config.aggression {
        return ResolvedBotProfile {
            level: config.level,
            style: None,
            variant: None,
            aggression,
            team_role,
            facets: ProfileFacets {
                economy_bias: 500,
                air_bias: 500,
                siege_bias: 500,
                support_bias: 500,
                commitment_bias: aggression,
            }
            .with_role(team_role),
        };
    }

    let style = config
        .style
        .unwrap_or_else(|| deal_named_style(scenario_seed, deal_player));
    let variant = mirrored_variant
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
    let (foundry_w, foundry_h) = BuildingKind::Foundry.stats().size;
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

    let (foundry_w, foundry_h) = BuildingKind::Foundry.stats().size;
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
}
