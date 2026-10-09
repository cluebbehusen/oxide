//! The shared command panel for whatever is selected.
//!
//! A building shows its portrait, production cards with sprites and
//! costs, and its queue as cancelable thumbnails; a harvester shows the
//! build palette and its order queue. Every card is a button routed
//! through the action its hotkey dispatches, and hovering any card raises
//! a tooltip: what it is, what it costs, how it fights, and the key that
//! does the same thing.

pub(crate) mod info;
mod upgrade;

use crate::action::{Action, BindingMap};
use crate::bot_label::{BotLabelStyle, bot_label};
use crate::game::Scene;
use crate::game::projection::{Program, Projection};
use crate::numeric;
use crate::numeric::Fit;
use crate::typography::entity_name;
use oxide_sim::stats::{BuildingKind, UnitKind, WeaponStats};
use oxide_sim::{BuildingId, Order};
use std::fmt::Write as _;

/// What a card wears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardIcon {
    /// A unit sprite (drawn in the human's faction colors).
    Unit(UnitKind),
    /// A building sprite.
    Building(BuildingKind, u8),
    /// A verb pictogram from the atlas's icon family.
    Verb(VerbIcon),
    /// A salvage pile: a wreck, or a scrap node.
    Salvage {
        /// Whether it is a wreck rather than a scrap node.
        wreck: bool,
    },
    /// An order chip that knows what it acts on: the subject's own
    /// sprite under a corner verb badge. Orders with no subject
    /// (Idle, Run, Advance, Hunt, Harvest) stay plain
    /// [`CardIcon::Verb`].
    Order {
        /// The machine or works the verb acts on.
        subject: OrderSubject,
        /// The verb, worn as a badge rather than the whole face.
        verb: VerbIcon,
        /// An unfinished site: translucent hull plus scaffold, the
        /// same language the world draws it in.
        ghost: bool,
    },
}

/// What an order chip is about, with the colors that subject actually
/// wears: an attack victim is not the panel owner's faction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderSubject {
    /// A machine: an attack victim.
    Unit(UnitKind, oxide_sim::Faction),
    /// Works: a site being raised, a patient, a strip job, a victim.
    Building(BuildingKind, oxide_sim::Faction),
}

/// The atlas's verb pictograms, in `Sprites::verb_icons` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbIcon {
    /// Everything halts.
    Stop,
    /// The oblivious walk.
    Move,
    /// The advance under fire.
    Advance,
    /// The strike burst.
    Attack,
    /// The loop.
    Patrol,
    /// The scrap pyramid.
    Harvest,
    /// The wrench.
    Build,
    /// The weld.
    Repair,
    /// Value coming back down.
    Salvage,
    /// The refusal cross.
    Cancel,
    /// The rally pennant.
    Rally,
    /// The three-beat wait.
    Idle,
    Run,
    Hunt,
}

/// What clicking a card does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardAction {
    /// Route through the action system, exactly like its hotkey.
    Dispatch(Action),
    /// Arm building placement (what the palette digit does).
    ArmBuild(BuildingKind),
    /// Arm the next world/minimap click as the selected producers' rally.
    ArmRally,
    /// Remove a queued unit from a producer (full refund).
    CancelQueue(BuildingId, u8),
    /// Cancel one selected factory product, preferring waiting work.
    CancelProduction(UnitKind),
    /// Cancel an unfinished site shown by a Harvester's Build order.
    CancelSite(BuildingId),
    /// Cancel one unstarted paid site across its assigned Harvester crew.
    CancelFound(BuildingKind, chassis::grid::TilePos),
    /// Remove one order from every selected own unit that still has it.
    CancelOrder {
        /// The unit whose chip was pressed.
        unit: oxide_sim::UnitId,
        /// Which order.
        key: oxide_sim::OrderKey,
        /// How many later orders of that unit's program share the key.
        from_end: u8,
    },
    /// Clear the selected producers' rally points.
    ClearRally,
    /// Lift a built own building one tier through its automatic rebuild.
    Upgrade,
    /// Abandon eligible selected fresh construction sites.
    ScrapSites,
    /// Set a transport's cargo down around where it hovers.
    UnloadHere(oxide_sim::UnitId),
    /// Narrow the selection to one kind (Ctrl, Shift, or a lit QUEUE
    /// removes it instead) — the mixed-army type strip.
    FilterKind(UnitKind),
    /// Close the build palette in one press, keeping the selection.
    ClosePalette,
    /// Empty the selected producers' queues, every job refunded in full.
    ClearQueues,
    /// Display only.
    None,
    /// A disabled card: pressing it explains why instead of acting.
    Refused,
}

impl CardAction {
    pub(crate) fn semantic(self) -> Option<Action> {
        match self {
            Self::Dispatch(action) => Some(action),
            Self::ArmBuild(kind) => Some(Action::Build(kind)),
            Self::ArmRally => Some(Action::SetRally),
            Self::ClearRally => Some(Action::ClearRally),
            Self::Upgrade => Some(Action::Upgrade),
            Self::UnloadHere(_) => Some(Action::Unload),
            _ => None,
        }
    }

    /// Whether pressing this dock chip throws away queued or started work:
    /// an order, a construction site, a planned foundation, or a production
    /// slot.
    pub(crate) fn discards_work(self) -> bool {
        matches!(
            self,
            Self::CancelOrder { .. }
                | Self::CancelSite(_)
                | Self::CancelFound(..)
                | Self::CancelQueue(..)
                | Self::CancelProduction(_)
        )
    }
}

/// One button (or display chip) on the panel.
pub struct Card {
    /// Face of the card.
    pub icon: CardIcon,
    /// Name shown in the tooltip header.
    pub title: String,
    /// Scrap cost, when the card buys something.
    pub cost: Option<u32>,
    /// The hotkey performing the same act, from the live bindings.
    pub hotkey: String,
    /// What clicking does.
    pub action: CardAction,
    /// Whether the card can act right now.
    pub enabled: bool,
    /// Why not, when disabled — surfaced in the tooltip.
    pub why: Option<String>,
    /// Tooltip body: description plus weapon lines.
    pub desc: Vec<String>,
    /// How far along the card's job is, 0-1, when it has one: the
    /// production head's bar and an order chip's own meter. The renderer
    /// draws this rather than reading the state itself.
    pub progress: Option<f32>,
}

/// Compact semantic mark paired with an always-visible capability fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityIcon {
    /// Weapon reach and damage against ground targets.
    Weapon,
    /// Weapon reach and damage against air targets.
    AirWeapon,
    /// Ground too close for the weapon to fire.
    DeadZone,
    /// Direct line of sight.
    Vision,
    /// Radar contact reach.
    Radar,
    /// Automatic repair reach.
    Repair,
    /// Economic support shared by Foundries and Extractors.
    EconomySupport,
}

/// The panel for the current selection.
pub struct Panel {
    pub(crate) info: info::SelectionInfo,
    /// Header line: name (and count for multi-selections).
    pub title: String,
    /// Summary for grouped selections.
    pub summary: String,
    /// Portrait icon.
    pub portrait: CardIcon,
    /// Whose colors the portrait and queue sprites wear: the selected
    /// entity's owner, not the viewer's.
    pub faction: oxide_sim::Faction,
    /// A mixed selection's unit-kind filters. Kept separate from
    /// command cards so choosing a roster slice can never crowd out a
    /// verb or make the verb row look like more selected units.
    pub roster: Vec<Card>,
    /// Command cards.
    pub cards: Vec<Card>,
    /// Queue thumbnails (production or orders).
    pub queue: Vec<Card>,
    /// Counts for a collective production dock; empty for individual queues.
    pub queue_groups: Vec<crate::production::QueueGroup>,
    /// What the queue strip is labeled. An order dock draws the subject's
    /// program, and the portrait beside it already names whose.
    pub queue_label: String,
    /// The Stop button above the dock: it halts the selection's orders, clears
    /// a defense's target preference, or empties production. Absent
    /// while nothing selected has anything to stop.
    pub stop: Option<Card>,
}

impl Panel {
    /// The card drawn in `row` at `index`.
    pub(crate) fn card(&self, row: crate::layout::CardRow, index: usize) -> Option<&Card> {
        match row {
            crate::layout::CardRow::Roster => self.roster.get(index),
            crate::layout::CardRow::Cards => self.cards.get(index),
            crate::layout::CardRow::Queue => self.queue.get(index),
            crate::layout::CardRow::Stop => self.stop.as_ref().filter(|_| index == 0),
        }
    }
}

/// The selection's subject: the unit whose program the dock, the
/// portrait, and the full-opacity breadcrumbs all describe, so those
/// surfaces agree. Majority kind first (a mixed army reads as its bulk,
/// not its lowest id), lowest id inside it as the deterministic
/// tie-break.
pub fn subject_unit(game: &Scene<'_>) -> Option<oxide_sim::UnitId> {
    let units: Vec<_> = game
        .presentation
        .selection
        .units
        .iter()
        .filter_map(|id| game.state.unit(*id))
        .collect();
    let mut counts: Vec<(UnitKind, usize)> = Vec::new();
    for u in &units {
        match counts.iter_mut().find(|(k, _)| *k == u.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((u.kind, 1)),
        }
    }
    let (majority, _) = counts
        .into_iter()
        .max_by_key(|&(k, n)| (n, std::cmp::Reverse(k.name())))?;
    units
        .iter()
        .filter(|u| u.kind == majority)
        .map(|u| u.id)
        .min()
}

/// The player-facing description per unit kind: the sim's own copy
/// ([`UnitKind::blurb`]), shared by the tooltip and the codex.
pub fn unit_flavor(kind: UnitKind) -> &'static str {
    kind.blurb()
}

/// The player-facing description per building kind
/// ([`BuildingKind::blurb`]).
pub fn building_flavor(kind: BuildingKind) -> &'static str {
    kind.blurb()
}

/// The figures a player weighs before buying a machine, on one line:
/// toughness, pace, build time, sight. Weapons get their own lines.
pub fn unit_stat_line(kind: UnitKind) -> String {
    let stats = kind.stats();
    let speed = stats.speed.to_num::<f32>() * oxide_sim::TICKS_PER_SECOND as f32;
    let build = stats.train_ticks as f32 / oxide_sim::TICKS_PER_SECOND as f32;
    format!(
        "{} hp | {speed:.1} tiles/s | {build:.1} s build | sight {}",
        stats.max_hp, stats.vision
    )
}

/// The figures a player weighs before raising a works, on one line:
/// toughness, build time, footprint, sight.
pub fn building_stat_line(kind: BuildingKind) -> String {
    let stats = kind.base_stats();
    let build = stats.construction.map_or(0.0, |c| {
        c.build_ticks as f32 / oxide_sim::TICKS_PER_SECOND as f32
    });
    let (width, height) = kind.size();
    format!(
        "{} hp | {build:.1} s build | {width}x{height} | sight {}",
        stats.max_hp, stats.vision
    )
}

/// Economy details shared by build cards and the codex.
pub fn building_economy_lines(kind: BuildingKind) -> Vec<String> {
    let radius = oxide_sim::stats::EXTRACTOR_SUPPORT_RADIUS;
    let remote = oxide_sim::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE;
    let supported = oxide_sim::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE;
    match kind {
        BuildingKind::Reclaimer => vec![format!(
            "Generates {} scrap/min. Upgrading to a Refinery raises output to {} scrap/min.",
            60 * u64::from(oxide_sim::TICKS_PER_SECOND) / oxide_sim::stats::RECLAIMER_PERIOD,
            60 * u64::from(oxide_sim::TICKS_PER_SECOND) / oxide_sim::stats::REFINERY_PERIOD,
        )],
        BuildingKind::Extractor => vec![
            format!(
                "Income: {remote} scrap/min remote; {supported} scrap/min with non-stacking support."
            ),
            format!(
                "Supported by one or more own completed Foundries within {radius} footprint tiles."
            ),
        ],
        BuildingKind::Foundry => vec![format!(
            "Supports own completed Extractors within {radius} footprint tiles: {remote} to {supported} scrap/min; additional Foundries do not stack."
        )],
        _ => Vec::new(),
    }
}

/// Current recurring output; harvesting and temporary recovery grants are separate.
pub(crate) fn building_income(game: &Scene<'_>, building: &oxide_sim::state::Building) -> u32 {
    if building.player != game.presentation.human || !building.built || building.hp == 0 {
        return 0;
    }
    if let Some(income) = game.state.extractor_income(building.id) {
        return income.scrap_per_minute();
    }
    let period = match building.kind {
        BuildingKind::Reclaimer if building.tier == 0 => oxide_sim::stats::RECLAIMER_PERIOD,
        BuildingKind::Reclaimer => oxide_sim::stats::REFINERY_PERIOD,
        BuildingKind::Foundry
            if game.state.current_tick() >= oxide_sim::stats::FOUNDRY_DRIP_START_TICK =>
        {
            oxide_sim::stats::FOUNDRY_DRIP_PERIOD
        }
        _ => return 0,
    };
    (60 * u64::from(oxide_sim::TICKS_PER_SECOND) / period).fit::<u32>()
}

fn weapon_line(weapon: &WeaponStats) -> String {
    let targets = match (
        weapon.targets.covers(oxide_sim::stats::Domain::Ground),
        weapon.targets.covers(oxide_sim::stats::Domain::Air),
    ) {
        (true, true) => "ground and air",
        (true, false) => "ground",
        (false, true) => "air",
        (false, false) => "nothing",
    };
    let flavor = if weapon.projectile.is_some() {
        " | projectile"
    } else if weapon.indirect {
        " | indirect"
    } else {
        ""
    };
    let splash = if weapon.splash.is_some() {
        " | splash"
    } else {
        ""
    };
    let range = if weapon.minimum_range > chassis::fx::Fx::ZERO {
        format!(
            "{:.1}-{:.1} tiles",
            weapon.minimum_range.to_num::<f32>(),
            weapon.range.to_num::<f32>()
        )
    } else {
        format!("{:.1} tiles", weapon.range.to_num::<f32>())
    };
    format!(
        "{} dmg | {range} | {targets}{flavor}{splash}",
        weapon.damage
    )
}

/// The compact mark shared by a weapon fact and its battlefield range.
/// Air-only weapons need a different silhouette, not just a quieter copy
/// of the ground ring, because the Sentinel exposes both at once.
pub(crate) fn weapon_capability_icon(weapon: &WeaponStats) -> CapabilityIcon {
    if weapon.targets.covers(oxide_sim::stats::Domain::Air)
        && !weapon.targets.covers(oxide_sim::stats::Domain::Ground)
    {
        CapabilityIcon::AirWeapon
    } else {
        CapabilityIcon::Weapon
    }
}

/// Human lines for a kind's weapons, from the stats table.
pub fn weapon_lines(kind: UnitKind) -> Vec<String> {
    kind.stats().weapons.iter().map(weapon_line).collect()
}

/// A building's weapon lines at `tier`, for the codex page.
pub fn building_weapon_lines(kind: BuildingKind, tier: u8) -> Vec<String> {
    kind.tier_stats(tier)
        .weapons
        .iter()
        .map(weapon_line)
        .collect()
}

pub(crate) fn tick_time_label(ticks: u32) -> String {
    let per_second = oxide_sim::TICKS_PER_SECOND;
    let tenths = ticks.saturating_mul(10).div_ceil(per_second);
    let whole = tenths / 10;
    if tenths.is_multiple_of(10) {
        format!("{whole}s")
    } else {
        format!("{whole}.{}s", tenths % 10)
    }
}

/// Compact build-time mark shown directly on a unit's training card.
pub fn unit_train_time_label(kind: UnitKind) -> String {
    tick_time_label(kind.stats().train_ticks)
}

fn unit_speed_label(kind: UnitKind) -> String {
    format!(
        "{:.1} tiles/sec",
        kind.stats().speed.to_num::<f32>() * oxide_sim::TICKS_PER_SECOND as f32
    )
}

/// The production dock's heading: the time left on everything queued,
/// or "Ready" while a finished head waits to leave.
fn production_queue_label(
    queue: &std::collections::VecDeque<UnitKind>,
    progress: u32,
) -> Option<String> {
    let head = queue.front()?;
    let head_ticks = head.stats().train_ticks;
    if progress >= head_ticks {
        return Some("Ready".to_string());
    }
    let later_ticks = queue
        .iter()
        .skip(1)
        .map(|kind| kind.stats().train_ticks)
        .sum::<u32>();
    Some(tick_time_label(head_ticks - progress + later_ticks))
}

fn bot_controller_label(game: &Scene<'_>, player: oxide_sim::PlayerId) -> Option<String> {
    let spec = game.scenario.players.get(usize::from(player.0))?;
    if !spec.bot {
        return None;
    }
    spec.bot_config
        .map(|config| bot_label(config.difficulty, config.stance, BotLabelStyle::Controller))
}

/// The subject an order chip may show, plus the lines that name it, for
/// own programs only. An ally's chips stay bare pictograms rather than
/// relying on what team sight shares, and an attack victim resolves
/// through the breadcrumbs' own fog gate, so the chip and the trail
/// agree.
fn order_subject(
    game: &Scene<'_>,
    order: &Order,
    projection: Option<&Projection>,
) -> Option<(OrderSubject, String, bool, Option<f32>)> {
    let faction_of = |p| game.state.player(p).faction;
    match order {
        Order::Build { site } => {
            let b = projection.map_or_else(
                || game.state.building(*site),
                |projection| projection.building(game.state, *site),
            )?;
            let ticks = b.stats().construction.map_or(1, |c| c.build_ticks).max(1);
            let frac = (b.progress as f32 / ticks as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Building(b.kind, faction_of(b.player)),
                entity_name(b.kind.tier_name(b.tier)),
                !b.built,
                Some(frac),
            ))
        }
        Order::Repair { building } | Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let frac = (b.hp as f32 / b.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Building(b.kind, faction_of(b.player)),
                entity_name(b.kind.tier_name(b.tier)),
                !b.built,
                Some(frac),
            ))
        }
        Order::Attack { target, .. } => match game
            .state
            .attack_objective(game.presentation.human, *target)?
        {
            oxide_sim::AttackTarget::RememberedBuilding(memory) => Some((
                OrderSubject::Building(memory.building_kind, faction_of(memory.owner)),
                entity_name(memory.building_kind.name()),
                false,
                None,
            )),
            oxide_sim::AttackTarget::Contact(id) => {
                let track = game.my_vision().track(id)?;
                if let Some(uid) = track.visible_unit {
                    let unit = game.state.unit(uid)?;
                    Some((
                        OrderSubject::Unit(unit.kind, faction_of(unit.player)),
                        entity_name(unit.kind.name()),
                        false,
                        None,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        },
        // A weld patient is own by construction — no fog gate needed,
        // and its meter is the wound closing.
        Order::ReturnCargo { foundry, .. } => {
            let building = game.state.building(*foundry)?;
            Some((
                OrderSubject::Building(building.kind, faction_of(building.player)),
                entity_name(building.kind.name()),
                false,
                None,
            ))
        }
        Order::RepairUnit { unit } => {
            let u = game.state.unit(*unit)?;
            let frac = (u.hp as f32 / u.kind.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Unit(u.kind, faction_of(u.player)),
                entity_name(u.kind.name()),
                false,
                Some(frac),
            ))
        }
        // A pending found's subject is the kind it will claim — drawn as
        // a ghost, since nothing stands yet.
        Order::Found { kind, .. } => Some((
            OrderSubject::Building(*kind, faction_of(game.presentation.human)),
            entity_name(kind.name()),
            true,
            None,
        )),
        Order::Idle
        | Order::Run { .. }
        | Order::Harvest { .. }
        | Order::Hunt { .. }
        | Order::Advance { .. }
        | Order::Board { .. }
        | Order::Unload { .. }
        | Order::Land { .. } => None,
    }
}

fn order_card(
    game: &Scene<'_>,
    order: &Order,
    active: bool,
    own: bool,
    projection: Option<&Projection>,
) -> Card {
    let (icon, title, desc): (VerbIcon, &str, &str) = match order {
        Order::Idle => (
            VerbIcon::Idle,
            "Idle",
            "Idle; armed units attack nearby enemies automatically.",
        ),
        Order::Run { .. } => (
            VerbIcon::Run,
            "Run",
            "Running without firing or engaging enemies.",
        ),
        Order::ReturnCargo { repair, .. } => (
            VerbIcon::Harvest,
            "Return cargo",
            if *repair {
                "Delivering scrap, then repairing the Foundry."
            } else {
                "Delivering scrap, then waiting at the Foundry."
            },
        ),
        Order::Harvest { .. } => (
            VerbIcon::Harvest,
            "Harvest",
            "Collecting scrap and returning it to a Foundry.",
        ),
        Order::Attack { .. } => (
            VerbIcon::Attack,
            "Attack",
            "Attacking the selected target while it remains known.",
        ),
        Order::Build { .. } => (
            VerbIcon::Build,
            "Build",
            "Constructing the selected building.",
        ),
        Order::Repair { .. } => (
            VerbIcon::Repair,
            "Repair",
            "Repairing a damaged building; consumes scrap.",
        ),
        Order::Hunt { .. } => (
            VerbIcon::Hunt,
            "Hunt",
            "Moving while engaging enemies along the route.",
        ),
        Order::Advance { .. } => (
            VerbIcon::Advance,
            "Advance",
            "Moving while the primary weapon fires at targets already in range; never chasing.",
        ),
        Order::Board { .. } => (
            VerbIcon::Move,
            "Board",
            "Walking to a transport and climbing aboard.",
        ),
        Order::Unload { .. } => (
            VerbIcon::Move,
            "Unload",
            "Flying to a drop point to set every carried machine down.",
        ),
        Order::Land { .. } => (
            VerbIcon::Move,
            "Landing",
            "Setting down on open ground; takes off again on the next order.",
        ),
        Order::Salvage { .. } => (
            VerbIcon::Salvage,
            "Salvage",
            "Stripping a building down for a partial refund.",
        ),
        Order::Found { .. } => (
            VerbIcon::Build,
            "Found",
            "Moving to a paid scaffold; its ground will be checked when visible.",
        ),
        Order::RepairUnit { .. } => (
            VerbIcon::Repair,
            "Weld",
            "Repairing a damaged unit; consumes scrap.",
        ),
    };
    let subject = if own {
        order_subject(game, order, projection)
    } else {
        None
    };
    let mut desc = vec![desc.to_string()];
    let (face, head, progress) = match subject {
        Some((subject, name, ghost, progress)) => {
            if let Some(line) = subject_detail(game, order, progress) {
                desc.push(line);
            }
            (
                CardIcon::Order {
                    subject,
                    verb: icon,
                    ghost,
                },
                format!("{title} - {name}"),
                progress,
            )
        }
        None => (
            CardIcon::Verb(icon),
            if matches!(
                order,
                Order::Attack {
                    target: oxide_sim::AttackTarget::Contact(_),
                    ..
                }
            ) {
                "Attack - Radar contact".into()
            } else {
                title.to_string()
            },
            None,
        ),
    };
    Card {
        icon: face,
        title: if active {
            format!("{head} (now)")
        } else {
            head
        },
        cost: None,
        hotkey: String::new(),
        action: CardAction::None,
        enabled: true,
        why: None,
        desc,
        progress,
    }
}

/// The chip for `program.orders[index]` of the dock's own subject. Pressing
/// it removes that order from the selection; an unbuilt site and a planned
/// one are cancelled outright instead, for every crew member.
fn own_order_card(
    game: &Scene<'_>,
    subject: oxide_sim::UnitId,
    program: &Program,
    index: usize,
    projection: &Projection,
) -> Card {
    let order = &program.orders[index];
    let mut card = order_card(game, order, index == 0, true, Some(projection));
    let unbuilt = match order {
        Order::Build { site } => projection
            .building(game.state, *site)
            .filter(|building| !building.built),
        _ => None,
    };
    match (order, unbuilt) {
        (Order::Build { .. }, Some(site)) => {
            // A site only the staged commands place has no settled id yet:
            // other seats' commands, or batches before its own, can take the
            // id it was projected with. It is named by kind and anchor.
            card.action = if game.state.building(site.id).is_some() {
                CardAction::CancelSite(site.id)
            } else {
                CardAction::CancelFound(site.kind, site.anchor)
            };
            card.desc.push(format!(
                "{} to cancel the site and recover its remaining value.",
                crate::platform::tap_or_click_capitalized(crate::platform::TOUCH_ONLY)
            ));
        }
        (Order::Found { kind, anchor }, _) => {
            card.action = CardAction::CancelFound(*kind, *anchor);
            card.desc.push(format!(
                "{} to cancel this planned site.",
                crate::platform::tap_or_click_capitalized(crate::platform::TOUCH_ONLY)
            ));
        }
        _ => {
            let human = game.presentation.human;
            if let Some(key) = order.key(game.state, human) {
                let later = program.orders[index + 1..]
                    .iter()
                    .filter(|later| later.key(game.state, human) == Some(key))
                    .count();
                card.action = CardAction::CancelOrder {
                    unit: subject,
                    key,
                    from_end: u8::try_from(later).unwrap_or(u8::MAX),
                };
            }
        }
    }
    card
}

/// The concrete second tooltip line for a subject-bearing order: how
/// far the job has come, in the units the verb is actually measured in.
fn subject_detail(game: &Scene<'_>, order: &Order, progress: Option<f32>) -> Option<String> {
    let pct = |f: f32| numeric::to_u32((f * 100.0).round());
    match order {
        Order::Build { .. } => Some(format!("{}% raised", pct(progress?))),
        Order::Repair { building } => {
            let b = game.state.building(*building)?;
            Some(format!("{}/{} hp", b.hp, b.stats().max_hp))
        }
        Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let cost = b.stats().construction.map_or(0, |c| c.cost);
            let left = u64::from(cost) * oxide_sim::stats::SALVAGE_REFUND_PERMILLE / 1000
                * u64::from(b.hp)
                / u64::from(b.stats().max_hp.max(1));
            Some(format!(
                "{}/{} hp | ~{left} scrap left",
                b.hp,
                b.stats().max_hp
            ))
        }
        Order::RepairUnit { unit } => {
            let u = game.state.unit(*unit)?;
            Some(format!("{}/{} hp", u.hp, u.kind.stats().max_hp))
        }
        _ => None,
    }
}

fn chord(bindings: &BindingMap, action: Action) -> String {
    bindings.labels(action)
}

/// The Stop button above the dock for whatever `action` stops.
fn stop_card(action: CardAction, hotkey: String, desc: &str) -> Card {
    Card {
        icon: CardIcon::Verb(VerbIcon::Stop),
        title: "Stop".into(),
        cost: None,
        hotkey,
        action,
        enabled: true,
        why: None,
        desc: vec![desc.into()],
        progress: None,
    }
}

/// Builds the selection panel against the live construction-menu state.
pub fn build_for_palette(
    game: &Scene<'_>,
    bindings: &BindingMap,
    build_menu_open: bool,
) -> Option<Panel> {
    let mut panel = build_panel(game, bindings, build_menu_open)?;
    panel.info = info::selection_info(game, &panel);
    Some(panel)
}

pub(crate) fn build_for_input(
    game: &Scene<'_>,
    bindings: &BindingMap,
    input: &crate::input::InputState,
) -> Option<Panel> {
    let mut panel = build_for_palette(game, bindings, input.construction_open())?;
    let construction_scrap = input
        .construction_open()
        .then(|| crate::input::available_construction_scrap(game, input));
    for card in &mut panel.cards {
        if let CardAction::ArmBuild(kind) = card.action {
            if game.state.prerequisites_met(game.presentation.human, kind) {
                let cost = kind.base_stats().construction.map_or(0, |stats| stats.cost);
                card.enabled = construction_scrap.unwrap_or(0) >= cost;
                card.why = (!card.enabled).then(|| format!("needs {cost} scrap"));
            }
            let category = crate::action::building_category(kind);
            let key = bindings.labels(Action::Build(kind));
            card.hotkey = if input.build_category == Some(category) {
                key
            } else if input.build_category.is_some() {
                String::new()
            } else {
                format!(
                    "{} > {}",
                    bindings.label(Action::BuildCategory(category)),
                    key
                )
            };
        }
    }
    if crate::platform::TOUCH_ONLY {
        strip_hotkeys(&mut panel);
    }
    Some(panel)
}

/// A touch-only build has no keys to name, so its cards carry none.
fn strip_hotkeys(panel: &mut Panel) {
    for card in panel
        .roster
        .iter_mut()
        .chain(&mut panel.cards)
        .chain(&mut panel.queue)
    {
        card.hotkey.clear();
    }
}

/// How a roster tile narrows the selection. The lit QUEUE toggle is
/// touch's Shift, so it drops the kind as Shift- and Ctrl-clicks do.
fn roster_filter_desc(touch_only: bool) -> Vec<String> {
    if touch_only {
        vec![
            "Tap: keep only this kind.".into(),
            "With QUEUE lit, tap drops this kind.".into(),
        ]
    } else {
        vec![
            "Click: keep only this kind.".into(),
            "Shift- or Ctrl-click: drop this kind.".into(),
        ]
    }
}

/// How the Patrol card collects and starts its route.
fn patrol_desc(touch_only: bool) -> &'static str {
    if touch_only {
        "Arm a looping route, tap waypoints, then tap Patrol again to start it."
    } else {
        "Arm a looping route, click waypoints, then press again to start it."
    }
}

/// How ground machines board a selected transport.
fn transport_load_desc(touch_only: bool) -> &'static str {
    if touch_only {
        "Select ground machines, then long-press the transport to load them."
    } else {
        "Right-click ground machines onto the transport to load them."
    }
}

/// An inspected salvage tile: read-only, with no cards to act on.
fn pile_panel(game: &Scene<'_>, tile: chassis::grid::TilePos) -> Option<Panel> {
    let salvage = game.known_salvage(tile)?;
    let wreck = matches!(salvage, crate::game::Salvage::Wreck(_));
    Some(Panel {
        info: info::SelectionInfo::default(),
        title: if wreck { "Wreck" } else { "Scrap pile" }.into(),
        summary: String::new(),
        portrait: CardIcon::Salvage { wreck },
        faction: game.state.player(game.presentation.human).faction,
        roster: Vec::new(),
        cards: Vec::new(),
        queue: Vec::new(),
        queue_groups: Vec::new(),
        queue_label: String::new(),
        stop: None,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "assembles every card and readout of the selection panel"
)]
fn build_panel(game: &Scene<'_>, bindings: &BindingMap, build_menu_open: bool) -> Option<Panel> {
    if let Some(tile) = game.presentation.selection.pile {
        return pile_panel(game, tile);
    }
    let selected_buildings: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|b| game.presentation.selection.buildings.contains(&b.id))
        .collect();
    if let Some(&first) = selected_buildings.first() {
        let owner = first.player;
        let plural = selected_buildings.len() > 1;
        let homogeneous = selected_buildings.iter().all(|b| b.kind == first.kind);
        let mut panel = Panel {
            info: info::SelectionInfo::default(),
            title: if !plural {
                entity_name(first.kind.tier_name(first.tier))
            } else if homogeneous {
                format!(
                    "{} x {}",
                    entity_name(first.kind.name()),
                    selected_buildings.len()
                )
            } else {
                format!("{} BUILDINGS", selected_buildings.len())
            },
            summary: String::new(),
            portrait: CardIcon::Building(first.kind, first.tier),
            faction: game.state.player(owner).faction,
            roster: Vec::new(),
            cards: Vec::new(),
            queue: Vec::new(),
            queue_groups: Vec::new(),
            queue_label: "Queue".into(),
            stop: None,
        };
        if selected_buildings
            .iter()
            .any(|b| b.player != game.presentation.human)
        {
            if !plural && !game.state.hostile(game.presentation.human, owner) {
                panel.cards.push(Card {
                    icon: CardIcon::Verb(VerbIcon::Idle),
                    title: "Ally building".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::None,
                    enabled: true,
                    why: None,
                    desc: vec!["Read-only: allied buildings cannot be controlled.".into()],
                    progress: None,
                });
            }
            return Some(panel);
        }
        let selected = crate::building_actions::SelectedBuildings::inspect(game);
        panel.cards = selected.cards(bindings);
        if plural {
            let offline = selected.buildings.iter().filter(|b| !b.built).count();
            let focused = selected
                .buildings
                .iter()
                .filter(|b| b.focus.is_some())
                .count();
            panel.summary = format!(
                "{} ready · {offline} offline",
                selected.buildings.len() - offline
            );
            if focused > 0 {
                let _ = write!(panel.summary, " · {focused} targeting");
            }
        } else if let Some(building) = selected.buildings.first() {
            if let Some(target) = building.focus {
                panel.queue_label = "Target preference".into();
                panel.queue.push(order_card(
                    game,
                    &Order::Attack {
                        target,
                        resume: None,
                        pursue: true,
                    },
                    true,
                    true,
                    None,
                ));
            } else {
                panel.queue_label = production_queue_label(&building.queue, building.progress)
                    .unwrap_or_else(|| "queue".into());
            }
            for (i, &kind) in building.queue.iter().enumerate() {
                let progress = (i == 0).then(|| {
                    (building.progress as f32 / kind.stats().train_ticks.max(1) as f32)
                        .clamp(0.0, 1.0)
                });
                panel.queue.push(Card {
                    icon: CardIcon::Unit(kind),
                    title: entity_name(kind.name()),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::CancelQueue(building.id, i.fit::<u8>()),
                    enabled: true,
                    why: None,
                    desc: vec![format!(
                        "{} to cancel; full refund.",
                        crate::platform::tap_or_click_capitalized(crate::platform::TOUCH_ONLY)
                    )],
                    progress,
                });
            }
        }
        panel.stop = if selected.buildings.iter().any(|b| b.focus.is_some()) {
            Some(stop_card(
                CardAction::Dispatch(Action::StopOrScrap),
                chord(bindings, Action::StopOrScrap),
                "Clear target preference; resume automatic fire.",
            ))
        } else if selected.accepts && selected.buildings.iter().any(|b| !b.queue.is_empty()) {
            Some(stop_card(
                CardAction::ClearQueues,
                String::new(),
                "Cancel every queued unit; full refund.",
            ))
        } else {
            None
        };
        let production = crate::production::Production::from_selected(selected);
        if production.homogeneous() {
            panel.cards.extend(production.cards(bindings));
            if plural {
                (panel.queue, panel.queue_groups) = production.collective_queue();
                panel.queue_label = "Combined production".into();
            }
        }
        return Some(panel);
    }
    if game.presentation.selection.units.is_empty() {
        return None;
    }
    let units: Vec<_> = game
        .presentation
        .selection
        .units
        .iter()
        .filter_map(|id| game.state.unit(*id))
        .collect();
    let subject_id = subject_unit(game)?;
    let first = units.iter().find(|u| u.id == subject_id)?;
    let owner = first.player;
    let has_builder = units.iter().any(|u| u.kind.stats().harvest.is_some());
    let has_welder = units.iter().any(|u| u.kind.stats().welder);
    let has_fighter = units.iter().any(|u| u.kind.stats().can_fight());
    let mut panel = Panel {
        info: info::SelectionInfo::default(),
        title: if units.len() == 1 {
            entity_name(first.kind.name())
        } else {
            format!("{} UNITS", units.len())
        },
        summary: if units.len() == 1 {
            String::new()
        } else {
            let (kinds, extra) = {
                let mut ks: Vec<UnitKind> = units.iter().map(|u| u.kind).collect();
                // dedup only folds neighbors, and a mixed selection
                // arrives in id order where equal kinds need not be.
                ks.sort_by_key(|k| k.name());
                ks.dedup();
                let extra = ks.len().saturating_sub(4);
                let named: Vec<String> = ks.iter().map(|k| entity_name(k.name())).take(4).collect();
                (named, extra)
            };
            if extra > 0 {
                format!("{} +{extra} more", kinds.join(", "))
            } else {
                kinds.join(", ")
            }
        },
        portrait: CardIcon::Unit(first.kind),
        faction: game.state.player(owner).faction,
        roster: Vec::new(),
        cards: Vec::new(),
        queue: Vec::new(),
        queue_groups: Vec::new(),
        queue_label: "Orders".to_string(),
        stop: None,
    };
    if owner != game.presentation.human {
        // Foreign units inspect read-only. Static weapon facts are safe
        // for any visible unit. An ally also shows its orders, while a
        // hostile's order state remains hidden because it reveals intent.
        let hostile = game.state.hostile(game.presentation.human, owner);
        if !hostile && units.len() == 1 {
            panel
                .queue
                .push(order_card(game, &first.order, true, false, None));
            for order in &first.queue {
                panel
                    .queue
                    .push(order_card(game, order, false, false, None));
            }
        }
        return Some(panel);
    }
    // The roster strip: a mixed army offers one counted chip per kind.
    // It has its own eight-chip budget, so every roster role stays
    // reachable without consuming command verbs.
    if units.len() > 1 {
        let mut counts: Vec<(UnitKind, usize)> = Vec::new();
        for u in &units {
            match counts.iter_mut().find(|(k, _)| *k == u.kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((u.kind, 1)),
            }
        }
        if counts.len() > 1 {
            // The counted portrait tiles below already name every kind.
            // Repeating the same list here makes long mixed selections run
            // into those tiles and gives the eye two competing summaries.
            panel.summary.clear();
            counts.sort_by_key(|(k, _)| k.name());
            for (kind, n) in counts.into_iter().take(8) {
                panel.roster.push(Card {
                    icon: CardIcon::Unit(kind),
                    title: format!("{} x{n}", entity_name(kind.name())),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::FilterKind(kind),
                    enabled: true,
                    why: None,
                    desc: roster_filter_desc(crate::platform::TOUCH_ONLY),
                    progress: None,
                });
            }
        }
    }
    // Run and Hunt only differ from a plain move for a machine
    // that can shoot. Patrol stays for everyone: an unarmed scout patrols.
    if has_fighter {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Run),
            title: "Run".into(),
            cost: None,
            hotkey: chord(bindings, Action::Run),
            action: CardAction::Dispatch(Action::Run),
            enabled: true,
            why: None,
            desc: vec![
                "Move to the selected ground without attacking".into(),
                "or acquiring targets.".into(),
            ],
            progress: None,
        });
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Hunt),
            title: "Hunt".into(),
            cost: None,
            hotkey: chord(bindings, Action::Hunt),
            action: CardAction::Dispatch(Action::Hunt),
            enabled: true,
            why: None,
            desc: vec![
                "Move to the selected ground while engaging enemies.".into(),
                "Machines stop and chase targets along the route.".into(),
            ],
            progress: None,
        });
    }
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Patrol),
        title: "Patrol".into(),
        cost: None,
        hotkey: chord(bindings, Action::Patrol),
        action: CardAction::Dispatch(Action::Patrol),
        enabled: true,
        why: None,
        desc: vec![
            patrol_desc(crate::platform::TOUCH_ONLY).into(),
            "Machines engage whatever they meet along the way.".into(),
        ],
        progress: None,
    });
    if has_builder {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Salvage),
            title: "Salvage".into(),
            cost: None,
            hotkey: chord(bindings, Action::Salvage),
            action: CardAction::Dispatch(Action::Salvage),
            enabled: true,
            why: None,
            desc: vec![
                "Select a completed friendly building to dismantle".into(),
                "for a partial refund. Foundries cannot be salvaged.".into(),
            ],
            progress: None,
        });
    }
    // The torch decides the Weld card, not the harvest kit: the Tender
    // welds without ever gathering.
    if has_welder {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Repair),
            title: "Weld".into(),
            cost: None,
            hotkey: chord(bindings, Action::RepairUnit),
            action: CardAction::Dispatch(Action::RepairUnit),
            enabled: true,
            why: None,
            desc: vec![
                "Select a damaged friendly ground unit to repair it.".into(),
                "Each restored hit point consumes scrap.".into(),
            ],
            progress: None,
        });
    }
    // A selected own transport offers its drop verb; the readout in
    // the info column shows what the sling holds.
    if units.len() == 1
        && first.player == game.presentation.human
        && first.kind.stats().transport_capacity > 0
    {
        let loaded = !first.cargo.is_empty();
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Move),
            title: "Unload here".into(),
            cost: None,
            hotkey: chord(bindings, Action::Unload),
            action: CardAction::UnloadHere(first.id),
            enabled: loaded,
            why: (!loaded).then(|| "the sling is empty".to_string()),
            desc: vec![
                "Sets every carried machine down on open ground around the airframe.".into(),
                transport_load_desc(crate::platform::TOUCH_ONLY).into(),
            ],
            progress: None,
        });
    }
    if has_builder {
        let scrap = game.state.player(game.presentation.human).scrap;
        let palette_key = chord(bindings, Action::ToggleBuildPalette);
        if build_menu_open {
            panel.cards.clear();
            panel.roster.clear();
            panel.title = "CONSTRUCTION".into();
            panel.summary.clear();
            panel.portrait = CardIcon::Verb(VerbIcon::Build);
        } else {
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Build),
                title: "Build".into(),
                cost: None,
                hotkey: palette_key,
                action: CardAction::Dispatch(Action::ToggleBuildPalette),
                enabled: true,
                why: None,
                desc: vec!["Open construction.".into()],
                progress: None,
            });
        }
        for kind in crate::action::BUILD_CATEGORIES
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .filter(|_| build_menu_open)
        {
            let cost = kind.base_stats().construction.map_or(0, |c| c.cost);
            // The same construction tech gate placement enforces: an
            // enabled card would only arm a ghost the sim refuses.
            let (enabled, why) = if !game.state.prerequisites_met(game.presentation.human, kind) {
                let need = kind
                    .base_stats()
                    .construction
                    .map(|c| c.requires)
                    .unwrap_or_default()
                    .iter()
                    .map(|k| entity_name(k.name()))
                    .collect::<Vec<_>>()
                    .join(", ");
                (false, Some(format!("needs a standing {need}")))
            } else if scrap < cost {
                (false, Some(format!("needs {cost} scrap")))
            } else {
                (true, None)
            };
            let mut desc = vec![building_flavor(kind).to_string(), building_stat_line(kind)];
            desc.extend(building_economy_lines(kind));
            panel.cards.push(Card {
                icon: CardIcon::Building(kind, 0),
                title: entity_name(kind.name()),
                cost: Some(cost),
                hotkey: format!(
                    "{} > {}",
                    bindings.label(Action::BuildCategory(crate::action::building_category(
                        kind
                    ))),
                    bindings.label(Action::Build(kind))
                ),
                action: CardAction::ArmBuild(kind),
                enabled,
                why,
                desc,
                progress: None,
            });
        }
        if build_menu_open {
            // Esc and B step back through placement and category first;
            // the card leaves the palette outright, which touch has no
            // other way to do without dropping the selection.
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Cancel),
                title: "Back".into(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::ClosePalette,
                enabled: true,
                why: None,
                desc: vec!["Close construction.".into()],
                progress: None,
            });
        }
    }
    // Return cargo comes and goes as workers load and unload, so it goes
    // last: its arrival never shifts a card a finger is reaching for.
    if !build_menu_open
        && units
            .iter()
            .any(|unit| unit.kind.stats().harvest.is_some() && unit.carrying > 0)
    {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Harvest),
            title: "Return cargo".into(),
            cost: None,
            hotkey: chord(bindings, Action::ReturnCargo),
            action: CardAction::Dispatch(Action::ReturnCargo),
            enabled: true,
            why: None,
            desc: vec!["Cancel current and queued work, deliver scrap to the nearest reachable Foundry, then stay there.".into()],
            progress: None,
        });
    }
    // The first unit's program: what it is doing and what comes next.
    // An idle unit with nothing queued contributes no chips, so the
    // orders dock vanishes instead of showing a lone "Idle" cell. It is
    // the program the staged commands will leave, so a chip pressed while
    // commands wait names an order they have not already removed.
    let projection = game.projection();
    if let Some(program) = projection
        .program(first.id)
        .filter(|program| !program.is_idle())
    {
        for index in 0..program.orders.len().min(8) {
            panel
                .queue
                .push(own_order_card(game, first.id, program, index, &projection));
        }
    }
    // The square answers for the whole selection, so it shows even when
    // the subject is idle and only another selected unit is busy.
    if units
        .iter()
        .any(|u| !matches!(u.order, Order::Idle) || !u.queue.is_empty())
    {
        panel.stop = Some(stop_card(
            CardAction::Dispatch(Action::StopOrScrap),
            chord(bindings, Action::StopOrScrap),
            "Clear orders; stand and auto-engage.",
        ));
    }
    Some(panel)
}

#[cfg(test)]
mod tests;
