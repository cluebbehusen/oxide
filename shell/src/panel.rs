//! The shared command panel: one HUD grammar for everything selected.
//!
//! Click a building and its cards appear — portrait, production cards
//! with sprites and costs, the queue as cancelable thumbnails. Click a
//! harvester and the *same* panel shows the build palette and its order
//! queue. Every card is a button routed through the exact action its
//! hotkey dispatches (keyboard stays first-class), and hovering any
//! card raises a tooltip: what it is, what it costs, how it fights, and
//! the key that does the same thing.

use crate::action::{Action, BindingMap};
use crate::game::Game;
use oxide_sim::stats::{BuildingKind, UnitKind, WeaponStats};
use oxide_sim::{BuildingId, Order};

/// What a card wears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardIcon {
    /// A unit sprite (drawn in the human's faction colors).
    Unit(UnitKind),
    /// A building sprite.
    Building(BuildingKind),
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
    /// Cancel an unfinished site shown by a Harvester's Build order.
    CancelSite(BuildingId),
    /// Cancel one unpaid logical site across its assigned Harvester crew.
    CancelFound(BuildingKind, chassis::grid::TilePos),
    /// Clear the selected producers' rally points.
    ClearRally,
    /// Lift a built own building one tier: the shell drafts the nearest
    /// own construction-capable machines as the crew.
    Upgrade(BuildingId),
    /// Set a transport's cargo down around where it hovers.
    UnloadHere(oxide_sim::UnitId),
    /// Narrow the selection to one kind (Ctrl-click removes it
    /// instead) — the mixed-army type strip.
    FilterKind(UnitKind),
    /// Display only.
    None,
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

/// Compact semantic mark paired with an always-visible combat fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatIcon {
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
}

/// One compact capability row in the selection panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombatFact {
    /// Symbol shared with the corresponding battlefield ring.
    pub icon: CombatIcon,
    /// Numeric capability details, without explaining the ring style.
    pub text: String,
}

/// The panel for the current selection.
pub struct Panel {
    /// Header line: name (and count for multi-selections).
    pub title: String,
    /// Sub-line: hp, status.
    pub sub: String,
    /// Portrait icon.
    pub portrait: CardIcon,
    /// Whose colors the portrait and queue sprites wear — the SELECTED
    /// entity's owner, not the viewer (an inspected Cupric ally must
    /// not draw in Ferrous rust).
    pub faction: oxide_sim::Faction,
    /// Static capability facts for a singly selected entity. These
    /// are drawn without a hover and deliberately contain no order,
    /// target, or cooldown state, so inspecting a visible enemy reveals
    /// capability without revealing intent.
    pub combat: Vec<CombatFact>,
    /// A mixed selection's unit-kind filters. Kept separate from
    /// command cards so choosing a roster slice can never crowd out a
    /// verb or make the verb row look like more selected units.
    pub roster: Vec<Card>,
    /// Command cards.
    pub cards: Vec<Card>,
    /// Queue thumbnails (production or orders).
    pub queue: Vec<Card>,
    /// What the queue strip is labeled — for order docks, WHOSE
    /// program it shows ("orders - harvester"), because the dock draws
    /// one unit's story while breadcrumbs draw many.
    pub queue_label: String,
}

/// The selection's SUBJECT: the unit whose program the dock, the
/// portrait, and the full-opacity breadcrumbs all describe — one rule,
/// so the surfaces can never disagree. Majority kind first (a mixed
/// army reads as its bulk, not its lowest id), lowest id inside it as
/// the deterministic tie-break.
pub fn subject_unit(game: &Game) -> Option<oxide_sim::UnitId> {
    let units: Vec<_> = game
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

/// One-line flavor per unit kind — tooltip and codex copy.
pub fn unit_flavor(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Harvester => "Collects scrap, constructs buildings, and repairs units.",
        UnitKind::Sentinel => "General-purpose unit with ground and anti-air weapons.",
        UnitKind::Scuttler => "Fast ground raider effective against exposed Harvesters.",
        UnitKind::Lancer => "Long-range ground sniper; vulnerable at close range.",
        UnitKind::Bombard => "Long-range artillery that needs allied vision to fire.",
        UnitKind::Flakhound => "Tracked anti-air unit.",
        UnitKind::Buzzard => "Heavy hovering gunship that attacks ground targets.",
        UnitKind::Talon => "Heavy air-superiority fighter.",
        UnitKind::Stinger => "Low-cost ground anti-air unit.",
        UnitKind::Darter => "Fast aircraft that attacks ground targets.",
        UnitKind::Wisp => "Fast interceptor that attacks air targets only.",
        UnitKind::Warden => "Tier-two line brawler: the wall that walks.",
        UnitKind::Tender => "Armored mobile welder: field sustain for long pushes.",
        UnitKind::Excavator => "Super-harvester: digs faster, hauls triple, builds at double pace.",
        UnitKind::Kestrel => "Unarmed scout flyer with far sight.",
        UnitKind::Gnat => "Unarmed scout flyer with far sight.",
        UnitKind::Shrike => "Heavy interceptor: the bomber's escort and its answer.",
        UnitKind::Sylph => "Heavy interceptor: lighter, quicker, hungrier.",
        UnitKind::Condor => "Strategic bomber: one enormous bomb per committed pass.",
        UnitKind::Moth => "Carpet bomber: lays a stick of six bombs along its run.",
        UnitKind::Breaker => "Tier-three assault walker built to crack fortress lines.",
        UnitKind::Avalanche => "Tier-three rocket battery: extreme reach, blind up close.",
        UnitKind::Skyhook => "Air transport: carries ground machines across anything.",
        UnitKind::Sapper => {
            "Walking charge: detonates against its target; devastating to structures."
        }
    }
}

/// One-line flavor per building kind.
pub fn building_flavor(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::Foundry => {
            "Trains Harvesters and Sentinels. Losing all Foundries eliminates you."
        }
        BuildingKind::Fabricator => "Unlocks advanced ground units and aircraft.",
        BuildingKind::Turret => "Static defense that attacks ground units.",
        BuildingKind::FlakTurret => "Static defense that attacks aircraft.",
        BuildingKind::Bastion => {
            "Long-range artillery emplacement; needs allied vision and cannot fire point-blank."
        }
        BuildingKind::Array => {
            "Reveals terrain within 9 tiles and detects hostile units within 16."
        }
        BuildingKind::Extractor => {
            "Restored strip miner: the strongest income in the game, rebuilt only on a derelict frame."
        }
        BuildingKind::Airworks => "Air production hall: trains every flyer.",
        BuildingKind::Crucible => {
            "The tier-three works: trains the heaviest machines and gates the deepest upgrades."
        }
        BuildingKind::Barricade => "Standing wall segment: blocks ground movement.",
        BuildingKind::ScuttleCharge => {
            "Buried charge: hidden from enemies until scouted; detonates under hostile machines."
        }
        BuildingKind::Reclaimer => "Generates 1 scrap every 1.5 seconds.",
        BuildingKind::RepairBay => {
            "Automatically repairs friendly ground units within 4 tiles. Repairs consume scrap."
        }
    }
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
pub(crate) fn weapon_combat_icon(weapon: &WeaponStats) -> CombatIcon {
    if weapon.targets.covers(oxide_sim::stats::Domain::Air)
        && !weapon.targets.covers(oxide_sim::stats::Domain::Ground)
    {
        CombatIcon::AirWeapon
    } else {
        CombatIcon::Weapon
    }
}

/// Human lines for a kind's weapons, from the stats table.
pub fn weapon_lines(kind: UnitKind) -> Vec<String> {
    kind.stats().weapons.iter().map(weapon_line).collect()
}

fn building_combat_lines(kind: BuildingKind, tier: u8) -> Vec<CombatFact> {
    let stats = kind.tier_stats(tier);
    let mut lines: Vec<CombatFact> = stats
        .weapons
        .iter()
        .map(|weapon| CombatFact {
            icon: weapon_combat_icon(weapon),
            text: weapon_line(weapon),
        })
        .collect();
    if let Some(minimum) = stats
        .weapons
        .iter()
        .map(|weapon| weapon.minimum_range)
        .find(|minimum| *minimum > chassis::fx::Fx::ZERO)
    {
        lines.push(CombatFact {
            icon: CombatIcon::DeadZone,
            text: format!("{:.1} tiles", minimum.to_num::<f32>()),
        });
    }
    if stats
        .weapons
        .iter()
        .any(|weapon| weapon.range.to_num::<f32>() > stats.vision as f32)
        || kind == BuildingKind::Array
    {
        lines.push(CombatFact {
            icon: CombatIcon::Vision,
            text: format!("{} tiles", stats.vision),
        });
    }
    if kind == BuildingKind::Array {
        lines.push(CombatFact {
            icon: CombatIcon::Radar,
            text: format!("{} tiles", oxide_sim::stats::RADAR_DETECT_RADIUS),
        });
    }
    if kind == BuildingKind::RepairBay {
        lines.push(CombatFact {
            icon: CombatIcon::Repair,
            text: format!(
                "{:.1} tiles",
                oxide_sim::stats::REPAIR_BAY_RADIUS.to_num::<f32>()
            ),
        });
    }
    lines
}

/// Always-visible combat facts for a selected unit.
pub fn combat_lines(kind: UnitKind) -> Vec<CombatFact> {
    let stats = kind.stats();
    let mut lines: Vec<_> = stats
        .weapons
        .iter()
        .map(|weapon| CombatFact {
            icon: weapon_combat_icon(weapon),
            text: weapon_line(weapon),
        })
        .collect();
    if stats
        .weapons
        .iter()
        .any(|weapon| weapon.range.to_num::<f32>() > stats.vision as f32)
    {
        lines.push(CombatFact {
            icon: CombatIcon::Vision,
            text: format!("{} tiles", stats.vision),
        });
    }
    lines
}

fn tick_time_label(ticks: u32) -> String {
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

fn bot_level_label(game: &Game, player: oxide_sim::PlayerId) -> Option<&'static str> {
    let spec = game.scenario.players.get(usize::from(player.0))?;
    if !spec.bot {
        return None;
    }
    spec.bot_config.map(|config| match config.level {
        oxide_sim::bot::Level::Easy => "Easy",
        oxide_sim::bot::Level::Medium => "Medium",
        oxide_sim::bot::Level::Hard => "Hard",
        oxide_sim::bot::Level::Expert => "Expert",
    })
}

fn foreign_sub(game: &Game, owner: oxide_sim::PlayerId, hostile: bool, detail: &str) -> String {
    let relation = if hostile { "hostile" } else { "ally" };
    if let Some(level) = bot_level_label(game, owner) {
        format!("{relation} | {level} | {detail}")
    } else {
        format!("{relation} | {detail}")
    }
}

/// The subject an order chip may show, plus the lines that name it —
/// OWN programs only. An ally's chips stay bare pictograms rather than
/// resting the panel on a claim about what team sight shares, and an
/// attack victim resolves through the breadcrumbs' own fog gate, so
/// the chip and the trail can never tell different stories.
fn order_subject(game: &Game, order: &Order) -> Option<(OrderSubject, String, bool, Option<f32>)> {
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
                b.kind.name().to_string(),
                !b.built,
                Some(frac),
            ))
        }
        Order::Repair { building } | Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let frac = (b.hp as f32 / b.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Building(b.kind, faction_of(b.player)),
                b.kind.name().to_string(),
                !b.built,
                Some(frac),
            ))
        }
        Order::Attack { target, .. } => {
            // The same gate the breadcrumb chase point uses: a victim
            // back in the fog is named by neither surface.
            let (subject, name, tile) = match target {
                oxide_sim::Target::Unit(uid) => {
                    let u = game.state.unit(*uid)?;
                    (
                        OrderSubject::Unit(u.kind, faction_of(u.player)),
                        u.kind.name().to_string(),
                        u.tile(),
                    )
                }
                oxide_sim::Target::Building(bid) => {
                    let b = game.state.building(*bid)?;
                    (
                        OrderSubject::Building(b.kind, faction_of(b.player)),
                        b.kind.name().to_string(),
                        b.anchor,
                    )
                }
            };
            (game.all_seeing() || game.my_vision().visible(tile))
                .then_some((subject, name, false, None))
        }
        // A weld patient is own by construction — no fog gate needed,
        // and its meter is the wound closing.
        Order::RepairUnit { unit } => {
            let u = game.state.unit(*unit)?;
            let frac = (u.hp as f32 / u.kind.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Unit(u.kind, faction_of(u.player)),
                u.kind.name().to_string(),
                false,
                Some(frac),
            ))
        }
        // A pending found's subject is the kind it will claim — drawn as
        // a ghost, since nothing stands yet.
        Order::Found { kind, .. } => Some((
            OrderSubject::Building(*kind, faction_of(game.human)),
            kind.name().to_string(),
            true,
            None,
        )),
        Order::Idle
        | Order::Move { .. }
        | Order::Harvest { .. }
        | Order::AttackMove { .. }
        | Order::Advance { .. }
        | Order::Board { .. }
        | Order::Unload { .. } => None,
    }
}

fn order_card(game: &Game, order: &Order, active: bool, own: bool) -> Card {
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
        Order::Harvest { .. } => (
            VerbIcon::Harvest,
            "Harvest",
            "Collecting scrap and returning it to a Foundry.",
        ),
        Order::Attack { .. } => (
            VerbIcon::Attack,
            "Attack",
            "Chasing one target until it is gone.",
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
        Order::Salvage { .. } => (
            VerbIcon::Salvage,
            "Salvage",
            "Stripping a building down for a partial refund.",
        ),
        Order::Found { .. } => (
            VerbIcon::Build,
            "Found",
            "Moving to the build site. Scrap is charged when construction begins.",
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
        None => (CardIcon::Verb(icon), title.to_string(), None),
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

fn own_order_card(game: &Game, order: &Order, active: bool) -> Card {
    let mut card = order_card(game, order, active, true);
    match order {
        Order::Build { site }
            if game
                .state
                .building(*site)
                .is_some_and(|building| !building.built) =>
        {
            card.action = CardAction::CancelSite(*site);
            card.desc
                .push("Click to cancel the site and recover its remaining value.".into());
        }
        Order::Found { kind, anchor } => {
            card.action = CardAction::CancelFound(*kind, *anchor);
            card.desc.push("Click to cancel this planned site.".into());
        }
        _ => {}
    }
    card
}

/// The concrete second tooltip line for a subject-bearing order: how
/// far the job has come, in the units the verb is actually measured in.
fn subject_detail(game: &Game, order: &Order, progress: Option<f32>) -> Option<String> {
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
    bindings
        .chord_for(action)
        .map(BindingMap::chord_label)
        .unwrap_or_default()
}

/// Builds the panel for the current selection, or `None` when nothing
/// warrants one. Pure: reads game + bindings, owns no state.
/// Panel construction with the open build-palette page — the shell
/// passes the live page so the card row matches what digits will do.
pub fn build_with_page(game: &Game, bindings: &BindingMap, build_page: usize) -> Option<Panel> {
    let faction = game.state.player(game.human).faction;
    let selected_buildings: Vec<_> = game
        .selection
        .buildings
        .iter()
        .filter_map(|id| game.state.building(*id))
        .collect();
    if selected_buildings.len() > 1 {
        let first = selected_buildings[0];
        let owner = first.player;
        let mut panel = Panel {
            title: format!("{} BUILDINGS", selected_buildings.len()),
            sub: {
                let mut kinds: Vec<BuildingKind> = selected_buildings
                    .iter()
                    .map(|building| building.kind)
                    .collect();
                kinds.sort_by_key(|kind| kind.name());
                kinds.dedup();
                format!("{} types", kinds.len())
            },
            portrait: CardIcon::Building(first.kind),
            faction: game.state.player(owner).faction,
            combat: Vec::new(),
            roster: Vec::new(),
            cards: Vec::new(),
            queue: Vec::new(),
            queue_label: "queue".to_string(),
        };
        if owner != game.human {
            let hostile = game.state.hostile(game.human, owner);
            panel.sub = foreign_sub(game, owner, hostile, &panel.sub);
            return Some(panel);
        }
        let producers: Vec<BuildingId> = selected_buildings
            .iter()
            .filter(|building| building.built && !building.stats().produces.is_empty())
            .map(|building| building.id)
            .collect();
        if !producers.is_empty() {
            let any_rally = producers.iter().any(|id| {
                game.state
                    .building(*id)
                    .is_some_and(|building| building.rally.is_some())
            });
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Rally),
                title: if any_rally {
                    "Reset rallies".into()
                } else {
                    "Set rallies".into()
                },
                cost: None,
                hotkey: String::new(),
                action: CardAction::ArmRally,
                enabled: true,
                why: None,
                desc: vec![format!(
                    "Set one destination for {} producers.",
                    producers.len()
                )],
                progress: None,
            });
            if any_rally {
                panel.cards.push(Card {
                    icon: CardIcon::Verb(VerbIcon::Rally),
                    title: "Clear rallies".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::ClearRally,
                    enabled: true,
                    why: None,
                    desc: vec!["Return new units to their producer doors.".into()],
                    progress: None,
                });
            }
        }
        return Some(panel);
    }
    if let Some(id) = game.selection.buildings.first().copied() {
        let building = game.state.building(id)?;
        let stats = building.stats();
        let owner = building.player;
        let mut panel = Panel {
            title: if building.tier > 0 {
                building.kind.tier_name(building.tier).to_uppercase()
            } else {
                building.kind.name().to_uppercase()
            },
            sub: format!("{}/{} hp", building.hp, stats.max_hp),
            portrait: CardIcon::Building(building.kind),
            faction: game.state.player(owner).faction,
            combat: building_combat_lines(building.kind, building.tier),
            roster: Vec::new(),
            cards: Vec::new(),
            queue: Vec::new(),
            queue_label: production_queue_label(&building.queue, building.progress)
                .unwrap_or_else(|| "queue".to_string()),
        };
        if owner != game.human {
            // Foreign buildings inspect read-only: an allied building says
            // whose they are; a hostile shows hp and kind, nothing
            // more — no queue chips, no cards, no rally, no reach
            // into anyone's production.
            let hostile = game.state.hostile(game.human, owner);
            panel.sub = foreign_sub(game, owner, hostile, &panel.sub);
            if !hostile {
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
        if !building.built {
            // A committed upgrade is not a scrappable site: the sim
            // refuses to demolish it, so the card must not offer to.
            if building.tier > 0 {
                panel.sub = format!("upgrading | {}", panel.sub);
                panel.cards.push(Card {
                    icon: CardIcon::Verb(VerbIcon::Cancel),
                    title: "Upgrading".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::None,
                    enabled: false,
                    why: Some("upgrades cannot be cancelled".into()),
                    desc: vec!["The works returns to service when the crew finishes.".into()],
                    progress: None,
                });
                return Some(panel);
            }
            panel.sub = format!("under construction | {}", panel.sub);
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Cancel),
                title: "Scrap site".into(),
                cost: None,
                hotkey: chord(bindings, Action::StopOrScrap),
                action: CardAction::Dispatch(Action::StopOrScrap),
                enabled: true,
                why: None,
                desc: vec!["Abandon the site for a partial refund.".into()],
                progress: None,
            });
            return Some(panel);
        }
        let scrap = game.state.player(game.human).scrap;
        if let Some(upgrade) = building.kind.upgrade_from(building.tier) {
            let next = building.kind.tier_name(building.tier + 1);
            let tech_ok = upgrade.requires.iter().all(|req| {
                game.state
                    .buildings()
                    .iter()
                    .any(|b| b.player == game.human && b.kind == *req && b.built)
            });
            let crew_available =
                game.state.units().iter().any(|u| {
                    u.player == game.human && u.hp > 0 && u.kind.stats().harvest.is_some()
                });
            let (enabled, why) = if !tech_ok {
                let need = upgrade
                    .requires
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>()
                    .join(", ");
                (false, Some(format!("needs a standing {need}")))
            } else if scrap < upgrade.cost {
                (false, Some(format!("needs {} scrap", upgrade.cost)))
            } else if !crew_available {
                // Activation drafts the nearest harvest-capable crew;
                // with none alive the click would silently do nothing.
                (false, Some("needs a harvest-capable crew".to_string()))
            } else {
                (true, None)
            };
            panel.cards.push(Card {
                icon: CardIcon::Building(building.kind),
                title: format!("Upgrade: {next}"),
                cost: Some(upgrade.cost),
                hotkey: String::new(),
                action: CardAction::Upgrade(building.id),
                enabled,
                why,
                desc: vec![
                    format!(
                        "Rebuilds this works as a {next} ({} ticks of labor).",
                        upgrade.build_ticks
                    ),
                    "The works goes offline until the crew finishes; upgrades cannot be cancelled."
                        .into(),
                ],
                progress: None,
            });
        }
        let queue_full = building.queue.len() >= oxide_sim::stats::QUEUE_CAP;
        if !stats.produces.is_empty() {
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Rally),
                title: if building.rally.is_some() {
                    "Reset rally".into()
                } else {
                    "Set rally".into()
                },
                cost: None,
                hotkey: String::new(),
                action: CardAction::ArmRally,
                enabled: true,
                why: None,
                desc: vec![
                    "Choose where newly trained units report.".into(),
                    "A scrap rally sends new Harvesters straight to work.".into(),
                ],
                progress: None,
            });
            if building.rally.is_some() {
                panel.cards.push(Card {
                    icon: CardIcon::Verb(VerbIcon::Rally),
                    title: "Clear rally".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::ClearRally,
                    enabled: true,
                    why: None,
                    desc: vec!["New units will remain near the producer.".into()],
                    progress: None,
                });
            }
        }
        for (i, &kind) in stats
            .produces
            .iter()
            .filter(|k| k.faction().is_none_or(|f| f == faction))
            .enumerate()
        {
            let cost = kind.stats().cost;
            // The tech gate the sim enforces at training time: an
            // enabled card whose click answers MissingPrerequisite is
            // a lie the disabled reason should have told instead.
            let missing_tech = kind.stats().requires.iter().find(|req| {
                !game
                    .state
                    .buildings()
                    .iter()
                    .any(|b| b.player == game.human && b.kind == **req && b.built)
            });
            let (enabled, why) = if queue_full {
                (false, Some("queue is full".to_string()))
            } else if let Some(req) = missing_tech {
                (false, Some(format!("needs a standing {}", req.name())))
            } else if scrap < cost {
                (false, Some(format!("needs {cost} scrap")))
            } else {
                (true, None)
            };
            let mut desc = vec![unit_flavor(kind).to_string()];
            desc.extend(weapon_lines(kind));
            panel.cards.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: Some(cost),
                hotkey: format!("{}", i + 1),
                action: CardAction::Dispatch(Action::TrainSlot(i as u8)),
                enabled,
                why,
                desc,
                progress: None,
            });
        }
        for (i, &kind) in building.queue.iter().enumerate() {
            // Only the head is being worked; the rest are prepaid ghosts.
            let progress = (i == 0).then(|| {
                let total = kind.stats().train_ticks.max(1);
                (building.progress as f32 / total as f32).clamp(0.0, 1.0)
            });
            panel.queue.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::CancelQueue(building.id, i as u8),
                enabled: true,
                why: None,
                desc: vec!["Click to cancel; full refund.".into()],
                progress,
            });
        }
        return Some(panel);
    }
    if game.selection.units.is_empty() {
        return None;
    }
    let units: Vec<_> = game
        .selection
        .units
        .iter()
        .filter_map(|id| game.state.unit(*id))
        .collect();
    let subject_id = subject_unit(game)?;
    let first = units.iter().find(|u| u.id == subject_id)?;
    let owner = first.player;
    let has_builder = units.iter().any(|u| u.kind.stats().harvest.is_some());
    let mut panel = Panel {
        title: if units.len() == 1 {
            first.kind.name().to_uppercase()
        } else {
            format!("{} UNITS", units.len())
        },
        sub: if units.len() == 1 {
            let mut sub = format!(
                "{}/{} hp | speed {}",
                first.hp,
                first.kind.stats().max_hp,
                unit_speed_label(first.kind)
            );
            let capacity = first.kind.stats().transport_capacity;
            // A hostile sling's load is intelligence the fog view
            // redacts from bots; the panel owes the player no more.
            if capacity > 0 && !game.state.hostile(game.human, owner) {
                let held: u8 = first
                    .cargo
                    .iter()
                    .map(|r| r.kind.stats().transport_size)
                    .sum();
                sub = format!("{sub} | sling {held}/{capacity}");
            }
            sub
        } else {
            let (kinds, extra) = {
                let mut ks: Vec<UnitKind> = units.iter().map(|u| u.kind).collect();
                // dedup only folds neighbors, and a mixed selection
                // arrives in id order where equal kinds need not be.
                ks.sort_by_key(|k| k.name());
                ks.dedup();
                let extra = ks.len().saturating_sub(4);
                let named: Vec<&str> = ks.iter().map(|k| k.name()).take(4).collect();
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
        combat: if units.len() == 1 {
            combat_lines(first.kind)
        } else {
            Vec::new()
        },
        roster: Vec::new(),
        cards: Vec::new(),
        queue: Vec::new(),
        queue_label: if units.len() == 1 {
            "orders".to_string()
        } else {
            // The dock shows ONE unit's program; say whose.
            format!("orders - {}", first.kind.name())
        },
    };
    if owner != game.human {
        // Foreign units inspect read-only. Static weapon facts are safe
        // for any visible unit. An ally also shows its orders, while a
        // hostile's order state remains hidden because it reveals intent.
        let hostile = game.state.hostile(game.human, owner);
        panel.sub = foreign_sub(game, owner, hostile, &panel.sub);
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
            panel.sub.clear();
            counts.sort_by_key(|(k, _)| k.name());
            for (kind, n) in counts.into_iter().take(8) {
                panel.roster.push(Card {
                    icon: CardIcon::Unit(kind),
                    title: format!("{} x{n}", kind.name()),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::FilterKind(kind),
                    enabled: true,
                    why: None,
                    desc: vec![
                        "Click: keep only this kind.".into(),
                        "Ctrl-click: drop this kind instead.".into(),
                    ],
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
    // the sub line shows what the sling holds.
    if units.len() == 1 && first.player == game.human && first.kind.stats().transport_capacity > 0 {
        let loaded = !first.cargo.is_empty();
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Move),
            title: "Unload here".into(),
            cost: None,
            hotkey: String::new(),
            action: CardAction::UnloadHere(first.id),
            enabled: loaded,
            why: (!loaded).then(|| "the sling is empty".to_string()),
            desc: vec![
                "Sets every carried machine down on open ground around the airframe.".into(),
                "Right-click ground machines onto the transport to load them.".into(),
            ],
            progress: None,
        });
    }
    if has_builder {
        let scrap = game.state.player(game.human).scrap;
        let palette_key = chord(bindings, Action::ToggleBuildPalette);
        for (i, &kind) in crate::input::build_page(build_page).iter().enumerate() {
            let cost = kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
            // The same construction tech gate placement enforces: an
            // enabled card would only arm a ghost the sim refuses.
            let (enabled, why) = if !game.state.prerequisites_met(game.human, kind) {
                let need = kind
                    .base_stats()
                    .construction
                    .map(|c| c.requires)
                    .unwrap_or_default()
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>()
                    .join(", ");
                (false, Some(format!("needs a standing {need}")))
            } else if scrap < cost {
                (false, Some(format!("needs {cost} scrap")))
            } else {
                (true, None)
            };
            panel.cards.push(Card {
                icon: CardIcon::Building(kind),
                title: kind.name().to_string(),
                cost: Some(cost),
                hotkey: format!("{palette_key},{}", i + 1),
                action: CardAction::ArmBuild(kind),
                enabled,
                why,
                desc: vec![building_flavor(kind).to_string()],
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
    use macroquad::prelude::vec2;
    use oxide_sim::{Command, PlayerCommand, Scenario};

    fn game() -> Game {
        Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).expect("skirmish builds")
    }

    fn human_foundry(game: &Game) -> oxide_sim::BuildingId {
        game.state
            .buildings()
            .iter()
            .find(|b| b.player == game.human)
            .expect("human foundry")
            .id
    }

    #[test]
    fn nothing_selected_builds_no_panel() {
        let game = game();
        assert!(build_with_page(&game, &BindingMap::classic(), 0).is_none());
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
            .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
            .expect("skirmish authors a sentinel")
            .id;
        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::AttackMove {
                units: vec![sentinel],
                goal: TilePos::new(20, 12),
                queue: false,
            },
        }]);
        game.selection.units = vec![sentinel];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
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
        game.selection.buildings = vec![foundry];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert_eq!(panel.title, "FOUNDRY");
        assert_eq!(
            panel.cards.len(),
            5,
            "four units plus the rally affordance (0.15: the Scuttler and \
             the Excavator train here)"
        );
        assert_eq!(panel.cards[0].title, "Set rally");
        assert_eq!(panel.cards[0].action, CardAction::ArmRally);
        assert_eq!(panel.cards[1].hotkey, "1");
        assert_eq!(panel.cards[1].cost, Some(50));
        assert_eq!(unit_train_time_label(UnitKind::Harvester), "5s");
        assert_eq!(unit_train_time_label(UnitKind::Sentinel), "7.5s");
        assert!(panel.cards[1].enabled, "150 scrap affords a harvester");
        assert_eq!(
            panel.cards[1].action,
            CardAction::Dispatch(Action::TrainSlot(0)),
            "the card IS its hotkey"
        );
        assert!(panel.queue.is_empty(), "nothing queued yet");
        // The harvester's card carries no weapon line; the sentinel's
        // carries both of its guns.
        assert!(!panel.cards[1].desc.iter().any(|l| l.contains("dmg")));
        assert!(panel.cards[2].desc.iter().any(|l| l.contains("dmg")));

        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        }]);
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("queued panel");
        assert_eq!(panel.queue_label, "queue 5s");
        game.state.tick(&[]);
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("progressing panel");
        assert_eq!(panel.queue_label, "queue 4.9s");
    }

    #[test]
    fn a_producer_always_exposes_set_reset_and_clear_rally_actions() {
        let mut game = game();
        let foundry = human_foundry(&game);
        game.selection.buildings = vec![foundry];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert!(
            panel
                .cards
                .iter()
                .any(|card| { card.title == "Set rally" && card.action == CardAction::ArmRally })
        );
        assert!(!panel.cards.iter().any(|card| card.title == "Clear rally"));

        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::SetRally {
                building: foundry,
                rally: Some(chassis::grid::TilePos::new(12, 8)),
            },
        }]);
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert!(
            panel
                .cards
                .iter()
                .any(|card| { card.title == "Reset rally" && card.action == CardAction::ArmRally })
        );
        assert!(
            panel.cards.iter().any(|card| {
                card.title == "Clear rally" && card.action == CardAction::ClearRally
            })
        );
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
        game.selection.buildings = game
            .state
            .buildings()
            .iter()
            .filter(|building| building.player == game.human)
            .map(|building| building.id)
            .collect();

        let panel =
            build_with_page(&game, &BindingMap::classic(), 0).expect("multi-building panel");
        assert_eq!(panel.title, "2 BUILDINGS");
        assert_eq!(panel.cards.len(), 1);
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
        game.selection.buildings = vec![turret];

        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("Turret panel");
        // 0.15: the one card a defense DOES carry is its tier upgrade.
        assert_eq!(
            panel.cards.len(),
            1,
            "the turret's only command is its upgrade"
        );
        assert!(
            matches!(panel.cards[0].action, CardAction::Upgrade(_)),
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
    fn poverty_and_capacity_disable_cards_with_reasons() {
        let mut scenario = Scenario::skirmish();
        // The bank must outlast the queue cap or poverty masks it.
        scenario.players[0].scrap = 500;
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
        let foundry = human_foundry(&game);
        game.selection.buildings = vec![foundry];
        // Queue harvesters until 50 scrap remains: the sentinel card
        // (75) must dim with the price named.
        for _ in 0..3 {
            game.state.tick(&[PlayerCommand {
                player: game.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        // 500 - 3x50 = 350: still rich, cards enabled, ghosts armed.
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert!(panel.cards[1].enabled);
        assert_eq!(panel.queue.len(), 3);
        assert_eq!(panel.queue[1].action, CardAction::CancelQueue(foundry, 1));
        // Fill to the sim's cap: every production card refuses.
        for _ in 0..oxide_sim::stats::QUEUE_CAP {
            game.state.tick(&[PlayerCommand {
                player: game.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        let queued = game.state.building(foundry).unwrap().queue.len();
        assert_eq!(queued, oxide_sim::stats::QUEUE_CAP, "the sim capped it");
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
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
            .find(|u| u.player == game.human && u.kind == oxide_sim::UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        game.selection.units = vec![harvester];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert_eq!(panel.title, "HARVESTER");
        assert_eq!(panel.cards[0].title, "Stop");
        assert_eq!(panel.cards[1].title, "Run");
        assert_eq!(panel.cards[2].title, "Attack-move");
        assert_eq!(panel.cards[3].title, "Patrol");
        assert!(
            panel.combat.is_empty(),
            "an unarmed unit needs no capability band"
        );
        assert_eq!(panel.sub, "60/60 hp | speed 2.5 tiles/sec");
        let builds: Vec<_> = panel
            .cards
            .iter()
            .filter(|c| matches!(c.action, CardAction::ArmBuild(_)))
            .collect();
        assert_eq!(builds.len(), crate::input::BUILD_PALETTE.len());
        assert!(
            builds[0].hotkey.starts_with("B,"),
            "palette cards teach their chord"
        );
        // An idle unit with nothing queued shows no order chips at all —
        // the dock only exists when there is a program to show.
        assert!(panel.queue.is_empty(), "idle shows no dock");
        // Give it a program: the strip appears, and stays display-only.
        game.state.tick(&[oxide_sim::PlayerCommand {
            player: game.human,
            command: oxide_sim::Command::AttackMove {
                units: vec![harvester],
                goal: chassis::grid::TilePos::new(8, 8),
                queue: false,
            },
        }]);
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
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
                if !game.state.can_place(game.human, kind, anchor) {
                    continue;
                }
                game.state.tick(&[PlayerCommand {
                    player: game.human,
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
            .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        (game, harvester)
    }

    #[test]
    fn build_chips_wear_the_works_they_are_raising() {
        let (mut game, harvester) = builder_game();
        let turret = place(&mut game, harvester, BuildingKind::Turret, false);
        let array = place(&mut game, harvester, BuildingKind::Array, true);
        game.selection.units = vec![harvester];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert_eq!(panel.queue.len(), 2, "two legs of one program");
        // Two Build chips that no longer look the same: each carries
        // its own works, ghosted while the site is still rising.
        let faction = game.state.player(game.human).faction;
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
            panel.queue[0].title.starts_with("Build - turret"),
            "{}",
            panel.queue[0].title
        );
        assert!(
            panel.queue[1].title.starts_with("Build - array"),
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
        let card = own_order_card(&game, &order, false);
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
        let card = order_card(&game, &dangling, true, true);
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
        game.selection.units = vec![harvester];
        let own = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert!(matches!(own.queue[0].icon, CardIcon::Order { .. }));
        let order = game.state.unit(harvester).expect("builder").order;
        let bare = order_card(&game, &order, true, false);
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
            assert!(supported(&panel.sub), "panel subtitle: {}", panel.sub);
            assert!(
                supported(&panel.queue_label),
                "panel queue label: {}",
                panel.queue_label
            );
            for fact in &panel.combat {
                assert!(supported(&fact.text), "panel capability: {}", fact.text);
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
            for fact in combat_lines(kind) {
                assert!(
                    supported(&fact.text),
                    "{} combat: {}",
                    kind.name(),
                    fact.text
                );
            }
        }
        for kind in buildings {
            assert!(supported(building_flavor(kind)), "{} flavor", kind.name());
            for fact in building_combat_lines(kind, 0) {
                assert!(
                    supported(&fact.text),
                    "{} combat: {}",
                    kind.name(),
                    fact.text
                );
            }
        }

        let mut foundry_game = game();
        foundry_game.selection.buildings = vec![human_foundry(&foundry_game)];
        assert_panel(
            &build_with_page(&foundry_game, &BindingMap::classic(), 0).expect("Foundry panel"),
        );

        let (mut builder_game, harvester) = builder_game();
        let site = place(&mut builder_game, harvester, BuildingKind::Array, false);
        builder_game.selection.units = vec![harvester];
        assert_panel(
            &build_with_page(&builder_game, &BindingMap::classic(), 0).expect("Harvester panel"),
        );
        builder_game.selection.units.clear();
        builder_game.selection.buildings = vec![site];
        assert_panel(
            &build_with_page(&builder_game, &BindingMap::classic(), 0).expect("site panel"),
        );
    }

    #[test]
    fn combat_facts_use_semantic_icons_instead_of_explaining_line_styles() {
        let bastion = building_combat_lines(BuildingKind::Bastion, 0);
        assert!(
            bastion.iter().any(|fact| {
                fact.icon == CombatIcon::Weapon && fact.text.contains("2.5-9.5 tiles")
            }),
            "{bastion:?}"
        );
        assert!(
            bastion
                .iter()
                .any(|fact| { fact.icon == CombatIcon::DeadZone && fact.text == "2.5 tiles" }),
            "{bastion:?}"
        );
        assert!(
            bastion
                .iter()
                .any(|fact| fact.icon == CombatIcon::Vision && fact.text == "6 tiles"),
            "{bastion:?}"
        );
        assert!(bastion.iter().all(|fact| {
            !fact.text.contains("dash")
                && !fact.text.contains("solid")
                && !fact.text.contains("amber")
                && !fact.text.contains("blue")
        }));

        let bombard = combat_lines(UnitKind::Bombard);
        assert!(
            bombard
                .iter()
                .any(|fact| fact.icon == CombatIcon::Vision && fact.text == "5 tiles"),
            "{bombard:?}"
        );
    }

    #[test]
    fn a_selected_bastion_keeps_the_dead_zone_symbol_visible() {
        let (mut game, harvester) = builder_game();
        let bastion = place(&mut game, harvester, BuildingKind::Bastion, false);
        game.selection.units.clear();
        game.selection.buildings = vec![bastion];

        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("Bastion panel");
        assert_eq!(panel.title, "BASTION");
        assert!(
            panel.combat.iter().any(|fact| {
                fact.icon == CombatIcon::Weapon && fact.text.contains("2.5-9.5 tiles")
            }),
            "{:?}",
            panel.combat
        );
        assert!(
            panel
                .combat
                .iter()
                .any(|fact| { fact.icon == CombatIcon::DeadZone && fact.text == "2.5 tiles" }),
            "{:?}",
            panel.combat
        );
    }

    #[test]
    fn a_single_unit_exposes_static_combat_facts_but_a_group_does_not() {
        let mut game = game();
        let sentinel = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
            .expect("starting sentinel")
            .id;
        game.selection.units = vec![sentinel];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert_eq!(
            panel.combat.len(),
            2,
            "the two weapons stay in the combat band"
        );
        assert!(
            panel
                .combat
                .iter()
                .any(|fact| fact.icon == CombatIcon::Weapon && fact.text.contains("ground"))
        );
        assert!(
            panel
                .combat
                .iter()
                .any(|fact| fact.icon == CombatIcon::AirWeapon && fact.text.contains("air")),
            "the anti-air range uses a targeted-aircraft mark"
        );
        assert!(panel.sub.ends_with("speed 2.2 tiles/sec"));
        assert!(
            panel
                .combat
                .iter()
                .filter(|fact| { matches!(fact.icon, CombatIcon::Weapon | CombatIcon::AirWeapon) })
                .all(|fact| fact.text.contains("dmg"))
        );
        assert!(panel.combat.iter().all(|fact| fact.text.contains("tiles")));
        assert!(panel.combat.iter().any(|fact| fact.text.contains("ground")));
        assert!(panel.combat.iter().any(|fact| fact.text.contains("air")));

        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        game.selection.units = vec![sentinel, harvester];
        let panel = build_with_page(&game, &BindingMap::classic(), 0).expect("panel");
        assert!(
            panel.combat.is_empty(),
            "mixed selections keep combat detail out of the command band"
        );
        assert_eq!(
            panel.roster.len(),
            2,
            "each selected kind gets one roster chip"
        );
        assert!(
            panel.sub.is_empty(),
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
