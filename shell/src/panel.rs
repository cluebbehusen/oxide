//! The shared command panel: one HUD grammar for everything selected.
//!
//! Click a building and its cards appear — portrait, production cards
//! with sprites and costs, the queue as cancelable thumbnails. Click a
//! harvester and the *same* panel shows the build palette and its order
//! queue. Every card is a button routed through the exact action its
//! hotkey dispatches (keyboard stays first-class), and hovering any
//! card raises a tooltip: what it is, what it costs, how it fights, and
//! the key that does the same thing.

pub(crate) mod info;
mod upgrade;

use crate::action::{Action, BindingMap};
use crate::bot_label::{BotLabelStyle, bot_label};
use crate::game::Scene;
use crate::typography::entity_name;
use oxide_sim::stats::{BuildingKind, UnitKind, WeaponStats};
use oxide_sim::{BuildingId, Order};

/// What a card wears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardIcon {
    /// A unit sprite (drawn in the human's faction colors).
    Unit(UnitKind),
    /// A building sprite.
    Building(BuildingKind, u8),
    /// A verb pictogram from the atlas's icon family.
    Verb(VerbIcon),
    /// An order chip that knows what it acts on: the subject's own
    /// sprite under a corner verb badge. Orders with no subject
    /// (Idle, Move, Advance, Attack-move, Harvest) stay plain
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

/// What an order chip is ABOUT, with the colors that subject actually
/// wears — an attack victim is not the panel owner's faction.
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
    /// The fighting march.
    AttackMove,
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
    /// Clear the selected producers' rally points.
    ClearRally,
    /// Lift a built own building one tier through its automatic rebuild.
    Upgrade,
    /// Abandon eligible selected fresh construction sites.
    ScrapSites,
    /// Set a transport's cargo down around where it hovers.
    UnloadHere(oxide_sim::UnitId),
    /// Narrow the selection to one kind (Ctrl-click removes it
    /// instead) — the mixed-army type strip.
    FilterKind(UnitKind),
    /// Display only.
    None,
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
    /// How far along the card's job is, 0-1, when it has one — the
    /// production head's bar and an order chip's own meter. The
    /// renderer draws this and never reaches back into the state for
    /// it; the panel model is the one description of the panel.
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
    /// Whose colors the portrait and queue sprites wear — the SELECTED
    /// entity's owner, not the viewer (an inspected Cupric ally must
    /// not draw in Ferrous rust).
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
    /// What the queue strip is labeled — for order docks, WHOSE
    /// program it shows ("orders - Harvester"), because the dock draws
    /// one unit's story while breadcrumbs draw many.
    pub queue_label: String,
}

/// The selection's SUBJECT: the unit whose program the dock, the
/// portrait, and the full-opacity breadcrumbs all describe — one rule,
/// so the surfaces can never disagree. Majority kind first (a mixed
/// army reads as its bulk, not its lowest id), lowest id inside it as
/// the deterministic tie-break.
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

/// The player-facing description per unit kind — the sim's own copy
/// ([`UnitKind::blurb`]), so the tooltip, the codex, and the source
/// never drift apart.
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
    format!(
        "{} hp | {build:.1} s build | {}x{} | sight {}",
        stats.max_hp, stats.size.0, stats.size.1, stats.vision
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
    (60 * u64::from(oxide_sim::TICKS_PER_SECOND) / period) as u32
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
    let flavor = if weapon.projectile {
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

fn production_queue_label(
    queue: &std::collections::VecDeque<UnitKind>,
    progress: u32,
) -> Option<String> {
    let head = queue.front()?;
    let head_ticks = head.stats().train_ticks;
    let ready = progress >= head_ticks;
    let later_ticks = queue
        .iter()
        .skip(1)
        .map(|kind| kind.stats().train_ticks)
        .sum::<u32>();
    if ready {
        return Some(if later_ticks == 0 {
            "queue ready".to_string()
        } else {
            format!("queue ready + {}", tick_time_label(later_ticks))
        });
    }
    let remaining = head_ticks - progress + later_ticks;
    Some(format!("queue {}", tick_time_label(remaining)))
}

fn bot_controller_label(game: &Scene<'_>, player: oxide_sim::PlayerId) -> Option<String> {
    let spec = game.scenario.players.get(usize::from(player.0))?;
    if !spec.bot {
        return None;
    }
    spec.bot_config
        .map(|config| bot_label(config.difficulty, config.stance, BotLabelStyle::Controller))
}

/// The subject an order chip may show, plus the lines that name it —
/// OWN programs only. An ally's chips stay bare pictograms rather than
/// resting the panel on a claim about what team sight shares, and an
/// attack victim resolves through the breadcrumbs' own fog gate, so
/// the chip and the trail can never tell different stories.
fn order_subject(
    game: &Scene<'_>,
    order: &Order,
) -> Option<(OrderSubject, String, bool, Option<f32>)> {
    let faction_of = |p| game.state.player(p).faction;
    match order {
        Order::Build { site } => {
            let b = game.state.building(*site)?;
            let ticks = b
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1)
                .max(1);
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
        | Order::Move { .. }
        | Order::Harvest { .. }
        | Order::AttackMove { .. }
        | Order::Advance { .. }
        | Order::Board { .. }
        | Order::Unload { .. }
        | Order::Land { .. } => None,
    }
}

fn order_card(game: &Scene<'_>, order: &Order, active: bool, own: bool) -> Card {
    let (icon, title, desc): (VerbIcon, &str, &str) = match order {
        Order::Idle => (
            VerbIcon::Idle,
            "Idle",
            "Idle; armed units attack nearby enemies automatically.",
        ),
        Order::Move { .. } => (
            VerbIcon::Move,
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
        Order::AttackMove { .. } => (
            VerbIcon::AttackMove,
            "Attack-move",
            "Moving while engaging enemies along the route.",
        ),
        Order::Advance { .. } => (
            VerbIcon::AttackMove,
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
        order_subject(game, order)
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

fn own_order_card(game: &Scene<'_>, order: &Order, active: bool) -> Card {
    let mut card = order_card(game, order, active, true);
    match order {
        Order::Build { site }
            if game
                .state
                .building(*site)
                .is_some_and(|building| !building.built) =>
        {
            card.action = CardAction::CancelSite(*site);
            card.desc.push(format!(
                "{} to cancel the site and recover its remaining value.",
                crate::platform::tap_or_click_capitalized(crate::platform::TOUCH_ONLY)
            ));
        }
        Order::Found { kind, anchor } => {
            card.action = CardAction::CancelFound(*kind, *anchor);
            card.desc.push(format!(
                "{} to cancel this planned site.",
                crate::platform::tap_or_click_capitalized(crate::platform::TOUCH_ONLY)
            ));
        }
        _ => {}
    }
    card
}

/// The concrete second tooltip line for a subject-bearing order: how
/// far the job has come, in the units the verb is actually measured in.
fn subject_detail(game: &Scene<'_>, order: &Order, progress: Option<f32>) -> Option<String> {
    let pct = |f: f32| (f * 100.0).round() as u32;
    match order {
        Order::Build { .. } => Some(format!("{}% raised", pct(progress?))),
        Order::Repair { building } => {
            let b = game.state.building(*building)?;
            Some(format!("{}/{} hp", b.hp, b.stats().max_hp))
        }
        Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let cost = b.stats().construction.map(|c| c.cost).unwrap_or(0);
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

pub(crate) fn build_for_input(game: &Scene<'_>, input: &crate::input::InputState) -> Option<Panel> {
    let mut panel = build_for_palette(game, &input.bindings, input.construction_open())?;
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
            let key = input.bindings.labels(Action::Build(kind));
            card.hotkey = if input.build_category == Some(category) {
                key
            } else if input.build_category.is_some() {
                String::new()
            } else {
                format!(
                    "{} > {}",
                    input.bindings.label(Action::BuildCategory(category)),
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

/// How a roster tile narrows the selection. Ctrl has no touch
/// equivalent, so touch offers only the keep-only filter.
fn roster_filter_desc(touch_only: bool) -> Vec<String> {
    if touch_only {
        vec!["Tap: keep only this kind.".into()]
    } else {
        vec![
            "Click: keep only this kind.".into(),
            "Ctrl-click: drop this kind instead.".into(),
        ]
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

/// The open construction palette's summary; the Back key closes it on
/// desktop, and touch closes it with the palette's own card.
fn construction_summary(back_key: &str, touch_only: bool) -> String {
    if touch_only {
        "Choose a building".to_string()
    } else {
        format!("Choose a building\n{back_key} to return")
    }
}

fn build_panel(game: &Scene<'_>, bindings: &BindingMap, build_menu_open: bool) -> Option<Panel> {
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
            queue_label: "queue".into(),
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
                panel.summary.push_str(&format!(" · {focused} targeting"));
            }
        } else if let Some(building) = selected.buildings.first() {
            if let Some(target) = building.focus {
                panel.queue_label = "target preference".into();
                panel.queue.push(order_card(
                    game,
                    &Order::Attack {
                        target,
                        resume: None,
                        pursue: true,
                    },
                    true,
                    true,
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
                    action: CardAction::CancelQueue(building.id, i as u8),
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
        let production = crate::production::Production::from_selected(selected);
        if production.homogeneous() {
            panel.cards.extend(production.cards(bindings));
            if plural {
                (panel.queue, panel.queue_groups) = production.collective_queue();
                panel.queue_label = "combined production".into();
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
        queue_label: if units.len() == 1 {
            "orders".to_string()
        } else {
            // The dock shows ONE unit's program; say whose.
            format!("orders - {}", entity_name(first.kind.name()))
        },
    };
    if owner != game.presentation.human {
        // Foreign units inspect read-only. Static weapon facts are safe
        // for any visible unit. An ally also shows its orders, while a
        // hostile's order state remains hidden because it reveals intent.
        let hostile = game.state.hostile(game.presentation.human, owner);
        if !hostile && units.len() == 1 {
            panel
                .queue
                .push(order_card(game, &first.order, true, false));
            for order in &first.queue {
                panel.queue.push(order_card(game, order, false, false));
            }
        }
        return Some(panel);
    }
    // The roster strip: a mixed army offers one chip per kind, counted.
    // Click keeps only that kind; Ctrl-click drops it — the two cuts
    // every RTS hand knows. It has its own eight-chip budget, so every
    // roster role stays reachable without consuming command verbs.
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
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Stop),
        title: "Stop".into(),
        cost: None,
        hotkey: chord(bindings, Action::StopOrScrap),
        action: CardAction::Dispatch(Action::StopOrScrap),
        enabled: true,
        why: None,
        desc: vec!["Clear orders; stand and auto-engage.".into()],
        progress: None,
    });
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Move),
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
        icon: CardIcon::Verb(VerbIcon::AttackMove),
        title: "Attack-move".into(),
        cost: None,
        hotkey: chord(bindings, Action::AttackMove),
        action: CardAction::Dispatch(Action::AttackMove),
        enabled: true,
        why: None,
        desc: vec![
            "Move to the selected ground while engaging enemies.".into(),
            "Machines stop and chase targets along the route.".into(),
        ],
        progress: None,
    });
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Patrol),
        title: "Patrol".into(),
        cost: None,
        hotkey: chord(bindings, Action::Patrol),
        action: CardAction::Dispatch(Action::Patrol),
        enabled: true,
        why: None,
        desc: vec![
            "Arm a looping route; press again to start it.".into(),
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
    if has_builder {
        let loaded = units
            .iter()
            .any(|unit| unit.kind.stats().harvest.is_some() && unit.carrying > 0);
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Harvest),
            title: "Return cargo".into(),
            cost: None,
            hotkey: chord(bindings, Action::ReturnCargo),
            action: CardAction::Dispatch(Action::ReturnCargo),
            enabled: loaded,
            why: (!loaded).then(|| "No scrap carried.".into()),
            desc: vec!["Cancel current and queued work, deliver scrap to the nearest reachable Foundry, then stay there.".into()],
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
            panel.summary =
                construction_summary(&bindings.label(Action::Back), crate::platform::TOUCH_ONLY);
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
                desc: vec!["Open construction. All buildings are shown together.".into()],
                progress: None,
            });
        }
        for kind in crate::action::BUILD_CATEGORIES
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .filter(|_| build_menu_open)
        {
            let cost = kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
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
    }
    // The first unit's program: what it is doing and what comes next.
    // An idle unit with nothing queued contributes no chips, so the
    // orders dock vanishes instead of showing a lone "Idle" cell.
    if !matches!(first.order, Order::Idle) || !first.queue.is_empty() {
        panel.queue.push(own_order_card(game, &first.order, true));
        for order in first.queue.iter().take(7) {
            panel.queue.push(own_order_card(game, order, false));
        }
    }
    Some(panel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;

    #[test]
    fn touch_copy_drops_keys_and_mouse_buttons_from_cards() {
        for line in roster_filter_desc(true) {
            crate::platform::assert_touch_copy(&line);
        }
        crate::platform::assert_touch_copy(transport_load_desc(true));
        crate::platform::assert_touch_copy(&construction_summary("Esc", true));
        assert_eq!(roster_filter_desc(false).len(), 2);
        assert_eq!(
            construction_summary("Esc", false),
            "Choose a building\nEsc to return"
        );
    }

    #[test]
    fn stripping_hotkeys_clears_every_card_row() {
        let mut game = game();
        game.presentation.selection.buildings = vec![human_foundry(&game)];
        let mut panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("hq panel");
        assert!(panel.cards.iter().any(|card| !card.hotkey.is_empty()));
        strip_hotkeys(&mut panel);
        for card in panel.roster.iter().chain(&panel.cards).chain(&panel.queue) {
            assert!(
                card.hotkey.is_empty(),
                "{} keeps {}",
                card.title,
                card.hotkey
            );
        }
    }

    fn stat<'a>(panel: &'a Panel, label: &str) -> &'a info::StatRow {
        panel
            .info
            .rows
            .iter()
            .find(|row| row.label == label)
            .unwrap()
    }
    use macroquad::prelude::vec2;
    use oxide_sim::{Command, PlayerCommand, Scenario};

    fn game() -> Game {
        Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).expect("skirmish builds")
    }

    fn human_foundry(game: &Game) -> oxide_sim::BuildingId {
        game.state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human)
            .expect("human foundry")
            .id
    }

    fn extractor_panel_game() -> Game {
        let mut scenario = Scenario::skirmish();
        let frames: Vec<_> = scenario
            .map
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.char_indices()
                    .filter(|(_, tile)| *tile == 'E')
                    .map(move |(x, _)| (x as i32, y as i32))
            })
            .collect();
        assert!(frames.len() >= 3, "fixture needs home and remote frames");
        let last = frames.len() - 1;
        scenario
            .buildings
            .extend(frames.into_iter().enumerate().map(|(index, (x, y))| {
                oxide_sim::scenario::BuildingSpec {
                    player: u8::from(index == last),
                    kind: BuildingKind::Extractor,
                    x,
                    y,
                }
            }));
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("Extractor fixture builds")
    }

    #[test]
    fn nothing_selected_builds_no_panel() {
        let game = game();
        assert!(build_for_palette(&game.view(), &BindingMap::classic(), false).is_none());
    }

    #[test]
    fn own_extractors_name_current_income_without_exposing_foreign_support() {
        let mut game = extractor_panel_game();
        let own_extractors: Vec<_> = game
            .state
            .buildings()
            .iter()
            .filter(|building| {
                building.player == game.presentation.human
                    && building.kind == BuildingKind::Extractor
            })
            .map(|building| building.id)
            .collect();
        let supported = own_extractors
            .iter()
            .copied()
            .find(|id| {
                game.state.extractor_income(*id) == Some(oxide_sim::ExtractorIncome::Supported)
            })
            .expect("a home Extractor is supported");
        let remote = own_extractors
            .iter()
            .copied()
            .find(|id| game.state.extractor_income(*id) == Some(oxide_sim::ExtractorIncome::Remote))
            .expect("a distant Extractor is remote");

        game.presentation.selection.buildings = vec![supported];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false)
            .expect("supported panel");
        assert_eq!(stat(&panel, "Income").value, "180 scrap/min");
        assert_eq!(stat(&panel, "Support").value, "Foundry");

        game.presentation.selection.buildings = vec![remote];
        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("remote panel");
        assert_eq!(stat(&panel, "Income").value, "120 scrap/min");
        assert_eq!(stat(&panel, "Support").value, "Remote");

        let foreign = game
            .state
            .buildings()
            .iter()
            .find(|building| {
                building.player != game.presentation.human
                    && building.kind == BuildingKind::Extractor
            })
            .expect("foreign Extractor")
            .id;
        assert_eq!(
            game.state.extractor_income(foreign),
            Some(oxide_sim::ExtractorIncome::Supported),
            "the fixture needs private dynamic support to hide"
        );
        game.presentation.selection.buildings = vec![foreign];
        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("foreign panel");
        assert!(
            panel
                .info
                .rows
                .iter()
                .all(|row| !matches!(row.label.as_str(), "Income" | "Support"))
        );
        assert_eq!(
            building_income(&game.view(), game.state.building(foreign).unwrap()),
            0
        );
        assert_eq!(stat(&panel, "Sight").value, "4 tiles");
    }

    #[test]
    fn recurring_income_tracks_real_output_upgrade_downtime_and_foundry_warmup() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 10_000;
        scenario.buildings.extend([
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 9,
                y: 3,
            },
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 12,
                y: 3,
            },
        ]);
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        let reclaimer = game
            .state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Reclaimer)
            .unwrap()
            .id;
        let foundry = human_foundry(&game);
        let rate =
            |game: &Game, id| building_income(&game.view(), game.state.building(id).unwrap());
        assert_eq!(rate(&game, reclaimer), 50);
        assert_eq!(rate(&game, foundry), 0);
        let initial = game.state.player(game.presentation.human).scrap;
        for _ in 0..1_200 {
            game.state.tick(&[]);
        }
        assert_eq!(
            game.state.player(game.presentation.human).scrap - initial,
            rate(&game, reclaimer)
        );
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::UpgradeBuilding {
                building: reclaimer,
            },
        }]);
        assert!(!game.state.building(reclaimer).unwrap().built);
        assert_eq!(rate(&game, reclaimer), 0);
        for _ in 0..300 {
            game.state.tick(&[]);
        }
        assert_eq!(game.state.building(reclaimer).unwrap().tier, 1);
        assert_eq!(rate(&game, reclaimer), 120);
        game.presentation.selection.buildings = vec![reclaimer];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).unwrap();
        assert_eq!(panel.title, "Refinery");
        assert_eq!(
            panel.portrait,
            CardIcon::Building(BuildingKind::Reclaimer, 1)
        );
        while game.state.current_tick() < oxide_sim::stats::FOUNDRY_DRIP_START_TICK - 1 {
            game.state.tick(&[]);
        }
        assert_eq!(rate(&game, foundry), 0);
        game.state.tick(&[]);
        assert_eq!(rate(&game, foundry), 20);
        let initial = game.state.player(game.presentation.human).scrap;
        for _ in 0..1_200 {
            game.state.tick(&[]);
        }
        assert_eq!(
            game.state.player(game.presentation.human).scrap - initial,
            rate(&game, foundry) + rate(&game, reclaimer)
        );
    }

    #[test]
    fn extractor_build_copy_names_both_rates_and_the_foundry_rule() {
        let extractor = building_economy_lines(BuildingKind::Extractor);
        assert!(
            extractor
                .iter()
                .any(|line| line.contains("120 scrap/min remote"))
        );
        assert!(
            extractor
                .iter()
                .any(|line| line.contains("180 scrap/min with non-stacking support"))
        );
        assert!(extractor.iter().any(|line| {
            line.contains("own completed Foundries") && line.contains("8 footprint tiles")
        }));

        let foundry = building_economy_lines(BuildingKind::Foundry);
        assert!(foundry.iter().any(|line| {
            line.contains("own completed Extractors")
                && line.contains("120 to 180 scrap/min")
                && line.contains("8 footprint tiles")
                && line.contains("do not stack")
        }));

        let mut game = game();
        let harvester = game
            .state
            .units()
            .iter()
            .find(|unit| unit.player == game.presentation.human && unit.kind == UnitKind::Harvester)
            .expect("starting Harvester")
            .id;
        game.presentation.selection.units = vec![harvester];
        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), true).expect("advanced builds");
        let extractor_card = panel
            .cards
            .iter()
            .find(|card| card.title == "Extractor")
            .expect("advanced palette contains Extractor");
        assert!(
            extractor_card
                .desc
                .iter()
                .any(|line| line.contains("120 scrap/min"))
        );
        assert!(
            extractor_card
                .desc
                .iter()
                .any(|line| line.contains("180 scrap/min"))
        );
    }

    #[test]
    fn scripted_opponents_show_their_difficulty_and_stance() {
        let mut game = game();
        game.scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::scripted(
            oxide_sim::scenario::BotDifficulty::Prime,
            oxide_sim::scenario::BotStance::Aggressive,
            91,
        ));
        assert_eq!(
            bot_controller_label(&game.view(), oxide_sim::PlayerId(1)),
            Some("Prime / Aggressive AI".to_string())
        );
    }

    #[test]
    fn verb_cards_wear_their_atlas_icons() {
        use chassis::grid::TilePos;
        use oxide_sim::UnitKind;
        let mut game = game();
        let sentinel = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
            .expect("skirmish authors a sentinel")
            .id;
        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::AttackMove {
                units: vec![sentinel],
                goal: TilePos::new(20, 12),
                queue: false,
            },
        }]);
        game.presentation.selection.units = vec![sentinel];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        let patrol = panel
            .cards
            .iter()
            .find(|c| c.title == "Patrol")
            .expect("patrol card");
        assert_eq!(patrol.icon, CardIcon::Verb(VerbIcon::Patrol));
        assert_eq!(patrol.hotkey, "R", "the tooltip chord stays live");
        let attack_move = panel
            .cards
            .iter()
            .find(|c| c.title == "Attack-move")
            .expect("attack-move card");
        assert_eq!(attack_move.action, CardAction::Dispatch(Action::AttackMove));
        assert_eq!(attack_move.hotkey, "F");
        let chip = &panel.queue[0];
        assert!(chip.title.starts_with("Attack-move"), "{}", chip.title);
        assert_eq!(
            chip.icon,
            CardIcon::Verb(VerbIcon::AttackMove),
            "chips wear pictograms, not letters that shadow chords"
        );
    }

    #[test]
    fn the_foundry_panel_speaks_its_roster() {
        let mut game = game();
        let foundry = human_foundry(&game);
        game.presentation.selection.buildings = vec![foundry];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(panel.title, "Foundry");
        assert_eq!(panel.cards.len(), 6, "four units plus two rally controls");
        assert_eq!(panel.cards[0].title, "Set rally");
        assert_eq!(panel.cards[0].action, CardAction::ArmRally);
        assert_eq!(panel.cards[2].hotkey, "Q");
        assert_eq!(panel.cards[2].cost, Some(50));
        assert_eq!(unit_train_time_label(UnitKind::Harvester), "5s");
        assert_eq!(unit_train_time_label(UnitKind::Sentinel), "7.5s");
        assert!(panel.cards[2].enabled, "150 scrap affords a harvester");
        assert_eq!(
            panel.cards[2].action,
            CardAction::Dispatch(Action::TrainSlot(0)),
            "the card IS its hotkey"
        );
        assert!(panel.queue.is_empty(), "nothing queued yet");
        // The harvester's card carries no weapon line; the sentinel's
        // carries both of its guns.
        assert!(!panel.cards[2].desc.iter().any(|l| l.contains("dmg")));
        assert!(panel.cards[3].desc.iter().any(|l| l.contains("dmg")));

        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        }]);
        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("queued panel");
        assert_eq!(panel.queue_label, "queue 5s");
        game.state.tick(&[]);
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false)
            .expect("progressing panel");
        assert_eq!(panel.queue_label, "queue 4.9s");
    }

    #[test]
    fn a_producer_always_exposes_set_reset_and_clear_rally_actions() {
        let mut game = game();
        let foundry = human_foundry(&game);
        game.presentation.selection.buildings = vec![foundry];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert!(
            panel
                .cards
                .iter()
                .any(|card| { card.title == "Set rally" && card.action == CardAction::ArmRally })
        );
        assert!(
            panel
                .cards
                .iter()
                .any(|card| card.title == "Clear rally" && !card.enabled)
        );

        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::SetRally {
                building: foundry,
                rally: Some(chassis::grid::TilePos::new(12, 8)),
            },
        }]);
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert!(
            panel
                .cards
                .iter()
                .any(|card| { card.title == "Reset rally" && card.action == CardAction::ArmRally })
        );
        assert!(panel.cards.iter().any(|card| {
            card.title == "Clear rally" && card.action == CardAction::ClearRally && card.enabled
        }));
    }

    #[test]
    fn a_multi_producer_panel_puts_shared_rally_before_production() {
        let mut scenario = Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 9,
            y: 3,
        });
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("fixture builds");
        game.presentation.selection.buildings = game
            .state
            .buildings()
            .iter()
            .filter(|building| building.player == game.presentation.human)
            .map(|building| building.id)
            .collect();

        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false)
            .expect("multi-building panel");
        assert_eq!(panel.title, "2 BUILDINGS");
        assert_eq!(panel.cards.len(), 2);
        assert_eq!(panel.cards[0].title, "Set rallies");
        assert_eq!(panel.cards[0].action, CardAction::ArmRally);
    }

    #[test]
    fn a_non_producer_never_offers_a_rally_action() {
        let mut scenario = Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 9,
            y: 3,
        });
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("fixture builds");
        let turret = game
            .state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .unwrap()
            .id;
        game.presentation.selection.buildings = vec![turret];

        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("Turret panel");
        assert_eq!(
            panel.cards.len(),
            2,
            "the turret offers Stop and its tier upgrade"
        );
        assert!(
            matches!(panel.cards[1].action, CardAction::Upgrade),
            "the turret's card lifts its tier"
        );
        assert!(
            panel
                .cards
                .iter()
                .all(|card| !matches!(card.action, CardAction::ArmRally | CardAction::ClearRally)),
            "a defense cannot rally units it never produces"
        );
    }

    #[test]
    fn an_upgrade_needs_no_harvester_and_explains_its_downtime() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 500;
        scenario
            .units
            .retain(|unit| unit.player != 0 || unit.kind != oxide_sim::UnitKind::Harvester);
        scenario.buildings.extend([
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 9,
                y: 3,
            },
            oxide_sim::scenario::BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 12,
                y: 3,
            },
        ]);
        let mut game =
            Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("upgrade fixture builds");
        let turret = game
            .state
            .buildings()
            .iter()
            .find(|building| building.kind == BuildingKind::Turret)
            .expect("fixture has a turret")
            .id;
        game.presentation.selection.buildings = vec![turret];

        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("turret panel");
        let upgrade = panel
            .cards
            .iter()
            .find(|card| matches!(card.action, CardAction::Upgrade))
            .expect("turret offers its upgrade");
        assert!(upgrade.enabled, "automatic upgrades need no crew");
        assert!(
            upgrade.desc.iter().any(|line| line.contains("Offline")),
            "the card explains the downtime: {:?}",
            upgrade.desc
        );

        game.state.tick(&[PlayerCommand {
            player: game.presentation.human,
            command: Command::UpgradeBuilding { building: turret },
        }]);
        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("upgrade panel");
        assert_eq!(panel.cards[0].title, "Upgrading");
        assert!(panel.cards[0].progress.is_some());
        assert!(
            panel.cards[0]
                .desc
                .iter()
                .all(|line| !line.contains("crew"))
        );
    }

    #[test]
    fn poverty_and_capacity_disable_cards_with_reasons() {
        let mut scenario = Scenario::skirmish();
        // The bank must outlast the queue cap or poverty masks it.
        scenario.players[0].scrap = 500;
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
        let foundry = human_foundry(&game);
        game.presentation.selection.buildings = vec![foundry];
        // Queue harvesters until 50 scrap remains: the sentinel card
        // (75) must dim with the price named.
        for _ in 0..3 {
            game.state.tick(&[PlayerCommand {
                player: game.presentation.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        // 500 - 3x50 = 350: still rich, cards enabled, ghosts armed.
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert!(panel.cards[2].enabled);
        assert_eq!(panel.queue.len(), 3);
        assert_eq!(panel.queue[1].action, CardAction::CancelQueue(foundry, 1));
        // Fill to the sim's cap: every production card refuses.
        for _ in 0..oxide_sim::stats::QUEUE_CAP {
            game.state.tick(&[PlayerCommand {
                player: game.presentation.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        let queued = game.state.building(foundry).unwrap().queue.len();
        assert_eq!(queued, oxide_sim::stats::QUEUE_CAP, "the sim capped it");
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(
            panel.queue.len(),
            oxide_sim::stats::QUEUE_CAP,
            "every paid queue slot remains inspectable and cancelable"
        );
        assert!(
            panel
                .cards
                .iter()
                .filter(|card| card.cost.is_some())
                .all(|card| !card.enabled)
        );
        assert!(
            panel
                .cards
                .iter()
                .filter(|card| card.cost.is_some())
                .all(|card| card.why.as_deref() == Some("queue is full")),
            "the reason names the cap, not the bank"
        );
    }

    #[test]
    fn the_harvester_panel_is_the_same_grammar() {
        let mut game = game();
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| {
                u.player == game.presentation.human && u.kind == oxide_sim::UnitKind::Harvester
            })
            .expect("starting harvester")
            .id;
        game.presentation.selection.units = vec![harvester];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(panel.title, "Harvester");
        assert_eq!(panel.cards[0].title, "Stop");
        assert_eq!(panel.cards[1].title, "Run");
        assert_eq!(panel.cards[2].title, "Attack-move");
        assert_eq!(panel.cards[3].title, "Patrol");
        assert!(
            !panel
                .info
                .rows
                .iter()
                .any(|row| matches!(row.label.as_str(), "Ground" | "Air"))
        );
        assert_eq!(panel.info.health, Some((60, 60)));
        assert_eq!(stat(&panel, "Speed").value, "2.5 tiles/s");
        assert!(panel.cards.iter().any(|card| card.title == "Build"));
        assert!(
            !panel
                .cards
                .iter()
                .any(|card| matches!(card.action, CardAction::ArmBuild(_)))
        );
        let construction = build_for_palette(&game.view(), &BindingMap::classic(), true).unwrap();
        assert_eq!(construction.cards.len(), 13);
        assert!(
            construction
                .cards
                .iter()
                .all(|card| matches!(card.action, CardAction::ArmBuild(_)))
        );
        // An idle unit with nothing queued shows no order chips at all —
        // the dock only exists when there is a program to show.
        assert!(panel.queue.is_empty(), "idle shows no dock");
        // Give it a program: the strip appears, and stays display-only.
        game.state.tick(&[oxide_sim::PlayerCommand {
            player: game.presentation.human,
            command: oxide_sim::Command::AttackMove {
                units: vec![harvester],
                goal: chassis::grid::TilePos::new(8, 8),
                queue: false,
            },
        }]);
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(panel.queue.len(), 1);
        assert_eq!(
            panel.queue[0].action,
            CardAction::None,
            "orders are display-only"
        );
    }

    /// Places `kind` on the first tile the sim accepts near the
    /// harvester, returns the site.
    fn place(
        game: &mut Game,
        builder: oxide_sim::UnitId,
        kind: BuildingKind,
        queue: bool,
    ) -> BuildingId {
        use chassis::grid::TilePos;
        let here = game.state.unit(builder).expect("builder").tile();
        let before: Vec<BuildingId> = game.state.buildings().iter().map(|b| b.id).collect();
        for dy in -6i32..=6 {
            for dx in -6i32..=6 {
                let (x, y) = (here.x + dx, here.y + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let anchor = TilePos::new(x, y);
                if !game.state.can_place(game.presentation.human, kind, anchor) {
                    continue;
                }
                game.state.tick(&[PlayerCommand {
                    player: game.presentation.human,
                    command: Command::Build {
                        units: vec![builder],
                        kind,
                        anchor,
                        queue,
                        defer: false,
                    },
                }]);
                if let Some(b) = game
                    .state
                    .buildings()
                    .iter()
                    .find(|b| !before.contains(&b.id) && b.kind == kind)
                {
                    return b.id;
                }
            }
        }
        panic!("no ground accepted a {}", kind.name());
    }

    fn builder_game() -> (Game, oxide_sim::UnitId) {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 5000;
        let game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        (game, harvester)
    }

    #[test]
    fn build_chips_wear_the_works_they_are_raising() {
        let (mut game, harvester) = builder_game();
        let turret = place(&mut game, harvester, BuildingKind::Turret, false);
        let array = place(&mut game, harvester, BuildingKind::Array, true);
        game.presentation.selection.units = vec![harvester];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(panel.queue.len(), 2, "two legs of one program");
        // Two Build chips that no longer look the same: each carries
        // its own works, ghosted while the site is still rising.
        let faction = game.state.player(game.presentation.human).faction;
        assert_eq!(
            panel.queue[0].icon,
            CardIcon::Order {
                subject: OrderSubject::Building(BuildingKind::Turret, faction),
                verb: VerbIcon::Build,
                ghost: true,
            }
        );
        assert_eq!(
            panel.queue[1].icon,
            CardIcon::Order {
                subject: OrderSubject::Building(BuildingKind::Array, faction),
                verb: VerbIcon::Build,
                ghost: true,
            }
        );
        assert!(
            panel.queue[0].title.starts_with("Build - Turret"),
            "{}",
            panel.queue[0].title
        );
        assert!(
            panel.queue[1].title.starts_with("Build - Array"),
            "{}",
            panel.queue[1].title
        );
        assert!(panel.queue[0].desc.iter().any(|l| l.contains("% raised")));
        assert!(
            panel.queue[0].progress.is_some(),
            "a site chip meters its rise"
        );
        assert_eq!(
            panel.queue[0].action,
            CardAction::CancelSite(turret),
            "the active site can be abandoned from its order chip"
        );
        assert_eq!(
            panel.queue[1].action,
            CardAction::CancelSite(array),
            "a queued paid site targets its own works"
        );
    }

    #[test]
    fn deferred_build_chips_cancel_their_logical_sites() {
        use chassis::grid::TilePos;

        let (game, _) = builder_game();
        let order = Order::Found {
            kind: BuildingKind::Bastion,
            anchor: TilePos::new(11, 7),
        };
        let card = own_order_card(&game.view(), &order, false);
        assert_eq!(
            card.action,
            CardAction::CancelFound(BuildingKind::Bastion, TilePos::new(11, 7))
        );
        assert!(card.desc.iter().any(|line| line.contains("planned site")));
    }

    #[test]
    fn a_chip_whose_subject_is_gone_falls_back_to_the_bare_verb() {
        // Orders outlive their subjects by a tick — the panel names
        // what it can find and never invents a silhouette.
        let (game, _) = builder_game();
        let dangling = Order::Repair {
            building: BuildingId(9999),
        };
        let card = order_card(&game.view(), &dangling, true, true);
        assert_eq!(card.icon, CardIcon::Verb(VerbIcon::Repair));
        assert_eq!(card.title, "Repair (now)");
        assert!(card.progress.is_none());
        assert_eq!(card.desc.len(), 1, "no detail line it cannot back up");
    }

    #[test]
    fn a_foreign_program_is_never_enriched() {
        // Enriching an inspected ally's chips would rest the panel on a
        // claim about what team sight shares; a teammate's dock says
        // the verb and nothing about what it acts on.
        let (mut game, harvester) = builder_game();
        place(&mut game, harvester, BuildingKind::Turret, false);
        game.presentation.selection.units = vec![harvester];
        let own = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert!(matches!(own.queue[0].icon, CardIcon::Order { .. }));
        let order = game.state.unit(harvester).expect("builder").order;
        let bare = order_card(&game.view(), &order, true, false);
        assert_eq!(bare.icon, CardIcon::Verb(VerbIcon::Build));
        assert_eq!(bare.title, "Build (now)");
        assert!(bare.progress.is_none());
    }

    #[test]
    fn weapon_lines_read_from_the_stats_table() {
        let sentinel = weapon_lines(oxide_sim::UnitKind::Sentinel);
        assert_eq!(sentinel.len(), 2, "main gun and the anti-air poke");
        assert!(sentinel[0].contains("dmg"));
        assert!(sentinel[0].contains("tiles"));
        assert!(sentinel[0].contains("ground"));
        assert!(sentinel[1].contains("air"));
        let bombard = weapon_lines(oxide_sim::UnitKind::Bombard);
        assert!(bombard[0].contains("projectile"));
        assert!(bombard[0].contains("splash"));
    }

    #[test]
    fn panel_copy_uses_only_supported_font_glyphs() {
        let units = [
            UnitKind::Harvester,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Bombard,
            UnitKind::Flakhound,
            UnitKind::Buzzard,
            UnitKind::Talon,
            UnitKind::Stinger,
            UnitKind::Darter,
            UnitKind::Wisp,
        ];
        let buildings = [
            BuildingKind::Foundry,
            BuildingKind::Fabricator,
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
            BuildingKind::Array,
            BuildingKind::Reclaimer,
            BuildingKind::RepairBay,
            BuildingKind::Extractor,
        ];
        let supported = |text: &str| text.is_ascii();
        let assert_card = |card: &Card| {
            assert!(supported(&card.title), "card title: {}", card.title);
            assert!(supported(&card.hotkey), "card hotkey: {}", card.hotkey);
            if let Some(why) = &card.why {
                assert!(supported(why), "card refusal: {why}");
            }
            for line in &card.desc {
                assert!(supported(line), "card description: {line}");
            }
        };
        let assert_panel = |panel: &Panel| {
            assert!(supported(&panel.title), "panel title: {}", panel.title);
            assert!(
                supported(&panel.summary),
                "panel subtitle: {}",
                panel.summary
            );
            assert!(
                supported(&panel.queue_label),
                "panel queue label: {}",
                panel.queue_label
            );
            for row in &panel.info.rows {
                assert!(
                    supported(&row.label) && supported(&row.value),
                    "panel stat: {row:?}"
                );
            }
            for status in &panel.info.status {
                assert!(supported(status), "panel status: {status}");
            }
            for card in panel.roster.iter().chain(&panel.cards).chain(&panel.queue) {
                assert_card(card);
            }
        };
        for kind in units {
            assert!(supported(unit_flavor(kind)), "{} flavor", kind.name());
            for line in weapon_lines(kind) {
                assert!(supported(&line), "{} weapon: {line}", kind.name());
            }
        }
        for kind in buildings {
            assert!(supported(building_flavor(kind)), "{} flavor", kind.name());
        }

        let mut foundry_game = game();
        foundry_game.presentation.selection.buildings = vec![human_foundry(&foundry_game)];
        assert_panel(
            &build_for_palette(&foundry_game.view(), &BindingMap::classic(), false)
                .expect("Foundry panel"),
        );

        let (mut builder_game, harvester) = builder_game();
        let site = place(&mut builder_game, harvester, BuildingKind::Array, false);
        builder_game.presentation.selection.units = vec![harvester];
        assert_panel(
            &build_for_palette(&builder_game.view(), &BindingMap::classic(), false)
                .expect("Harvester panel"),
        );
        builder_game.presentation.selection.units.clear();
        builder_game.presentation.selection.buildings = vec![site];
        assert_panel(
            &build_for_palette(&builder_game.view(), &BindingMap::classic(), false)
                .expect("site panel"),
        );
    }

    #[test]
    fn a_selected_bastion_shows_its_minimum_and_maximum_range() {
        let (mut game, harvester) = builder_game();
        let bastion = place(&mut game, harvester, BuildingKind::Bastion, false);
        game.presentation.selection.units.clear();
        game.presentation.selection.buildings = vec![bastion];

        let panel =
            build_for_palette(&game.view(), &BindingMap::classic(), false).expect("Bastion panel");
        assert_eq!(panel.title, "Bastion");
        assert_eq!(stat(&panel, "Range").value, "2.5-9.5 tiles");
        assert_eq!(stat(&panel, "Sight").value, "6 tiles");
        assert_eq!(
            stat(&panel, "Ground").icon,
            Some(info::StatIcon::Capability(CapabilityIcon::Weapon))
        );
    }

    #[test]
    fn a_single_unit_exposes_static_combat_facts_but_a_group_does_not() {
        let mut game = game();
        let sentinel = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Sentinel)
            .expect("starting sentinel")
            .id;
        game.presentation.selection.units = vec![sentinel];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert_eq!(stat(&panel, "Ground").value, "10 dmg/hit");
        assert_eq!(
            stat(&panel, "Ground").icon,
            Some(info::StatIcon::Capability(CapabilityIcon::Weapon))
        );
        assert_eq!(stat(&panel, "Air").value, "4 dmg/hit");
        assert_eq!(
            stat(&panel, "Air").icon,
            Some(info::StatIcon::Capability(CapabilityIcon::AirWeapon))
        );
        assert_eq!(stat(&panel, "Speed").value, "2.2 tiles/s");

        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        game.presentation.selection.units = vec![sentinel, harvester];
        let panel = build_for_palette(&game.view(), &BindingMap::classic(), false).expect("panel");
        assert!(
            panel.info.rows.is_empty(),
            "mixed selections keep combat detail out of the command band"
        );
        assert_eq!(
            panel.roster.len(),
            2,
            "each selected kind gets one roster chip"
        );
        assert!(
            panel.summary.is_empty(),
            "the counted roster tiles replace the redundant kind list"
        );
        assert!(
            panel
                .cards
                .iter()
                .all(|card| !matches!(card.action, CardAction::FilterKind(_))),
            "roster filters cannot consume command-card capacity"
        );
    }

    #[test]
    fn production_queue_time_counts_partial_head_and_marks_a_blocked_spawn_ready() {
        let queue = std::collections::VecDeque::from([UnitKind::Harvester, UnitKind::Sentinel]);
        assert_eq!(
            production_queue_label(&queue, 25).as_deref(),
            Some("queue 11.3s"),
            "75 head ticks plus 150 queued ticks"
        );
        assert_eq!(
            production_queue_label(&queue, UnitKind::Harvester.stats().train_ticks).as_deref(),
            Some("queue ready + 7.5s")
        );
        assert_eq!(
            production_queue_label(
                &std::collections::VecDeque::from([UnitKind::Harvester]),
                UnitKind::Harvester.stats().train_ticks,
            )
            .as_deref(),
            Some("queue ready")
        );
        assert!(production_queue_label(&std::collections::VecDeque::new(), 0).is_none());
    }
}
