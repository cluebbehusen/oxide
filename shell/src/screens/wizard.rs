//! The New Match flow: the map browser grid, then the match setup
//! screen (seat cards grouped by team beside a large who-is-where
//! preview) for every map size. Enter-Enter from the grid launches the
//! map as authored.
//!
//! Every answer lands in the draft's per-seat vector, and `launch()`
//! reads only that.

use crate::bot_label::{difficulty_name, stance_name};
use crate::game::SoundKind;
use crate::menu::{PreviewCache, ScenarioEntry, discover_scenarios};
use crate::nav::{Axis, Nav, step_line};
use crate::numeric;
use crate::numeric::Fit;
use crate::press::{Fed, Press};
use crate::screens::browser::{Browser, Out as BrowserOut};
use anyhow::{Context, Result};
use macroquad::prelude::{
    Color, DrawTextureParams, Rect, Vec2, draw_circle, draw_circle_lines, draw_rectangle,
    draw_rectangle_lines, draw_text, draw_texture_ex, measure_text, vec2,
};
use oxide_protocol::{Key, RawEvent};
use oxide_sim::Scenario;
use oxide_sim::scenario::{BotDifficulty, BotStance};
use std::path::PathBuf;

use crate::theme::{SURFACE_MENU, TEXT_DANGER, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TITLE};

/// One seat's editable choices in the draft.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SeatPlan {
    /// Player-facing skill rung for this opponent. It remains attached to the
    /// chair if the human moves elsewhere; launch ignores it only for the
    /// chair the human ultimately takes.
    pub difficulty: BotDifficulty,
    /// Player-facing strategic posture for this opponent.
    pub stance: BotStance,
    /// Whether another machine plays this chair; hosting the match lets
    /// it join. Like the bot choices, launch ignores it for the human's
    /// own chair.
    pub remote: bool,
    /// Faction chip (feeds [`faction_override`]): 0 keeps the map's
    /// authored roster. The human's own card carries this too.
    pub faction_choice: usize,
    /// Team chip (feeds [`team_override`]): 0 is FFA — the seat stands
    /// alone — and `k` is Team `k`. [`NewMatchDraft::set_scenario`]
    /// seeds it from the map's authored teams, so the bare default is
    /// only right for maps that author none. Carried on every card,
    /// the human's included: teams regroup seats, never retint them.
    pub team_choice: usize,
}

/// The faction a chip value forces onto its seat; `None` keeps the
/// map's authored faction.
pub fn faction_override(choice: usize) -> Option<oxide_sim::Faction> {
    match choice {
        1 => Some(oxide_sim::Faction::Ferrous),
        2 => Some(oxide_sim::Faction::Cupric),
        _ => None,
    }
}

/// The scenario team a chip value writes onto its seat: `None` (FFA)
/// puts the seat on its own team; `Team k` becomes the 0-based id the
/// sim densifies by first appearance at build.
pub fn team_override(choice: usize) -> Option<u8> {
    choice.checked_sub(1).map(|team| team.fit::<u8>())
}

/// The team chip's display label, aligned with [`team_override`].
pub fn team_chip_label(choice: usize) -> String {
    match choice {
        0 => "FFA".to_string(),
        k => format!("Team {k}"),
    }
}

/// The team chip a seat opens on: its authored team shown as the
/// dense first-appearance ordinal (`Team 1`, `Team 2`, ...) — the same
/// normalization the sim applies at build — or FFA when the seat
/// authors none.
fn default_team_choice(scenario: &Scenario, seat: usize) -> usize {
    let Some(team) = scenario.players.get(seat).and_then(|p| p.team) else {
        return 0;
    };
    let mut seen: Vec<u8> = Vec::new();
    for player in &scenario.players {
        if let Some(t) = player.team
            && !seen.contains(&t)
        {
            seen.push(t);
        }
    }
    seen.iter().position(|t| *t == team).map_or(0, |i| i + 1)
}

/// Fresh per-seat plans for a map: every choice at its default, the team
/// chips seeded from the authored teams.
fn authored_seat_plans(scenario: &Scenario) -> Vec<SeatPlan> {
    (0..scenario.players.len())
        .map(|seat| {
            let bot = scenario.players[seat].bot_config.unwrap_or_default();
            SeatPlan {
                difficulty: bot.difficulty,
                stance: bot.stance,
                team_choice: default_team_choice(scenario, seat),
                ..SeatPlan::default()
            }
        })
        .collect()
}

/// Whether the draft groups every seat onto one team — the sim's
/// `OneTeam` build refusal (nobody to fight), caught here so Start can
/// say why instead of failing the launch. All-FFA is the opposite
/// extreme and always legal: every seat stands alone.
fn draft_one_team(draft: &NewMatchDraft) -> bool {
    let Some(scenario) = draft.scenario.as_deref() else {
        return false;
    };
    let n = scenario.players.len();
    if n < 2 {
        return false;
    }
    let mut choices = (0..n).map(|i| draft.seats.get(i).map_or(0, |p| p.team_choice));
    let Some(first) = choices.next() else {
        return false;
    };
    first != 0 && choices.all(|c| c == first)
}

/// The faction a seat will actually run: its chip override, or the
/// map's authored roster.
pub fn effective_faction(
    scenario: &Scenario,
    draft: &NewMatchDraft,
    seat: usize,
) -> oxide_sim::Faction {
    draft
        .seats
        .get(seat)
        .and_then(|p| faction_override(p.faction_choice))
        .unwrap_or(scenario.players[seat].faction)
}

/// The name a seat will actually play under: the authored name run
/// through the launcher's own retint rule when a faction chip
/// overrides the roster. Lives beside [`effective_faction`] so the
/// card's disc and its label agree. Duplicate-name ordinals are added
/// at launch; the preview shows the pre-ordinal name.
pub fn effective_name(scenario: &Scenario, draft: &NewMatchDraft, seat: usize) -> String {
    let spec = &scenario.players[seat];
    oxide_sim::scenario::retinted_name(
        &spec.name,
        spec.faction,
        effective_faction(scenario, draft, seat),
    )
}

/// Everything New Match has chosen so far. The draft outlives every
/// screen transition: backing from any step to the map list and
/// forward again re-offers each earlier answer instead of forgetting
/// it.
#[derive(Default)]
pub struct NewMatchDraft {
    /// The loaded map, once picked.
    pub scenario: Option<Box<Scenario>>,
    /// The picked map's path (`None` = the embedded skirmish), keyed by
    /// path so the browser's section sort can never move the remembered
    /// highlight onto a different map.
    pub scenario_path: Option<PathBuf>,
    /// Which chair the human takes (index into the scenario's players).
    pub seat_choice: usize,
    /// One plan per seat, aligned with the scenario's player list and
    /// re-derived whenever the scenario changes — Back from an 8-seat
    /// map to a 2-seat map must not leave a stale seat 7. The human's
    /// own row is inert at launch.
    pub seats: Vec<SeatPlan>,
}

impl NewMatchDraft {
    /// Installs a picked map. Re-entering the same map keeps every
    /// earlier answer, so the draft survives Back; a different map resets
    /// the seats and the chair, so no chair index carries silently onto a
    /// map with different seats.
    pub fn set_scenario(&mut self, scenario: Scenario, path: Option<PathBuf>) {
        let same_map = self.scenario.is_some() && self.scenario_path == path;
        let count = scenario.players.len();
        let defaults = authored_seat_plans(&scenario);
        if same_map {
            if self.seats.len() < count {
                self.seats.extend_from_slice(&defaults[self.seats.len()..]);
            } else {
                self.seats.truncate(count);
            }
            self.seat_choice = self.seat_choice.min(count.saturating_sub(1));
        } else {
            self.seats = defaults;
            self.seat_choice = 0;
        }
        self.scenario = Some(Box::new(scenario));
        self.scenario_path = path;
    }
}

/// The setup cards' faction chip values, aligned with
/// [`faction_override`].
const FACTION_CHIP_ITEMS: [&str; 3] = ["Auto", "Ferrous", "Cupric"];
const MIN_TOUCH_TARGET: f32 = 44.0;
const COMPACT_PAGE_ITEMS: usize = 5;

/// The setup screen's coaching line. The keyboard hint follows the
/// cursor; taps move the cursor anyway, so the touch hint is one line.
fn setup_hint(one_team: bool, on_start: bool, cell: Cell, touch_only: bool) -> &'static str {
    match (touch_only, one_team) {
        (true, true) => "every seat is on one team, nobody to fight - tap a TEAM chip to regroup",
        (true, false) => "tap a seat to take it - tap a chip to change it",
        (false, true) => {
            "every seat is on one team, nobody to fight - regroup a TEAM chip - {back} back"
        }
        (false, false) if on_start => "{confirm} starts the match - {back} back",
        (false, false) => match cell {
            Cell::Difficulty => "{confirm} cycles difficulty - {left}/{right} move - {back} back",
            Cell::Stance => "{confirm} cycles stance - {left}/{right} move - {back} back",
            Cell::Faction | Cell::Team => {
                "{confirm} cycles the chip - {left}/{right} move - {back} back"
            }
            Cell::Seat => {
                "{confirm} takes this seat - {left}/{right} reach difficulty, stance, faction, and team - {back} back"
            }
        },
    }
}

fn start_label(draft: &NewMatchDraft) -> &'static str {
    if draft_hosts(draft) {
        "Host match"
    } else {
        "Start match"
    }
}

/// Which wizard screen is up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// The map browser grid.
    Map,
    /// Match setup, every map size: seat cards by team, Start, and a
    /// live map.
    Setup,
}

/// What a wizard frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still asking.
    Stay,
    /// Backed all the way out to the front door.
    Home,
    /// Every question answered: the caller launches from the draft.
    Launch,
}

/// The wizard: current step, the browser grid, and the setup cursor.
pub struct Wizard {
    /// Which screen is up.
    pub step: Step,
    /// Discovered scenario entries, section-sorted.
    pub entries: Vec<ScenarioEntry>,
    /// The map grid's state.
    pub browser: Browser,
    /// Setup cursor over the DISPLAY order: seats grouped by team,
    /// then the Start button.
    pub setup_sel: usize,
    /// Which cell of the selected seat card the cursor is on: 0 the
    /// seat itself, then its controls left to right: 1 difficulty,
    /// 2 stance, 3 faction, and 4 team. The two bot controls are absent
    /// from the human's row.
    pub setup_cell: Cell,
    /// Setup zone armed by a press; activation on release inside the
    /// same zone.
    setup_press: Press<SetupZone>,
    /// The corner Back button: Home from the grid, the grid from setup.
    back: crate::button::BackButton,
    /// Compact setup page. Full-height layouts always clamp this to zero.
    setup_page: usize,
}

/// One collision-free team key per seat: an authored id stays itself;
/// an omitted seat uses its own index lifted above the whole u8 range,
/// so no authored id can alias it and every pass that groups seats
/// derives the identical key.
fn seat_team_keys(scenario: &Scenario) -> Vec<u16> {
    scenario
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| p.team.map_or(256 + i.fit::<u16>(), u16::from))
        .collect()
}

/// Seats in DISPLAY order: grouped by team (first appearance), seat
/// order within — the setup screen's visual order and its cursor's
/// walking order are the same list.
pub fn seat_display_order(scenario: &Scenario) -> Vec<usize> {
    let keys = seat_team_keys(scenario);
    let mut teams: Vec<u16> = Vec::new();
    for &k in &keys {
        if !teams.contains(&k) {
            teams.push(k);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for team in &teams {
        for (i, &k) in keys.iter().enumerate() {
            if k == *team {
                order.push(i);
            }
        }
    }
    order
}

/// The setup screen's frame geometry, a pure function of the map and
/// window: seat cards on the left grouped under team headings, the
/// Start button beneath them, the preview panel filling the right.
pub struct SetupLayout {
    /// Team headings and their text rects.
    pub headings: Vec<(String, Rect)>,
    /// One card per DISPLAY position (see [`seat_display_order`]);
    /// `None` while the card sits on another compact page.
    pub cards: Vec<Option<CardRects>>,
    /// The Start button; `None` while it sits on another compact page.
    pub start: Option<Rect>,
    /// Where the map preview draws.
    pub preview: Rect,
    /// Previous compact page control, when another page precedes this one.
    pub page_prev: Option<Rect>,
    /// Next compact page control, when another page follows this one.
    pub page_next: Option<Rect>,
    /// Current zero-based compact page.
    pub page: usize,
    /// Number of compact pages; one means no pagination chrome is needed.
    pub page_count: usize,
    /// Half-open protocol item range actually drawn on this page.
    pub visible_range: [usize; 2],
}

/// One control on a seat card, left to right as the keyboard walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// The card itself: Enter takes the seat.
    Seat,
    Difficulty,
    Stance,
    Faction,
    Team,
}

impl Cell {
    /// Every cell, left to right.
    pub const ALL: [Self; 5] = [
        Self::Seat,
        Self::Difficulty,
        Self::Stance,
        Self::Faction,
        Self::Team,
    ];

    fn index(self) -> usize {
        match self {
            Self::Seat => 0,
            Self::Difficulty => 1,
            Self::Stance => 2,
            Self::Faction => 3,
            Self::Team => 4,
        }
    }
}

/// Where one seat card and its controls sit. Your own card has no
/// difficulty or stance chip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardRects {
    /// The whole card.
    pub card: Rect,
    /// The seat zone left of the chips.
    pub seat: Rect,
    pub difficulty: Option<Rect>,
    pub stance: Option<Rect>,
    pub faction: Rect,
    pub team: Rect,
}

impl CardRects {
    /// Where `cell` sits, if this card has it.
    pub fn cell(&self, cell: Cell) -> Option<Rect> {
        match cell {
            Cell::Seat => Some(self.seat),
            Cell::Difficulty => self.difficulty,
            Cell::Stance => self.stance,
            Cell::Faction => Some(self.faction),
            Cell::Team => Some(self.team),
        }
    }

    /// A fingertip's target for `cell`: the card's full height, reaching
    /// halfway to the neighboring controls.
    fn touch_cell(&self, cell: Cell) -> Option<Rect> {
        let rect = self.cell(cell)?;
        let index = cell.index();
        let left = Cell::ALL[..index]
            .iter()
            .rev()
            .find_map(|candidate| self.cell(*candidate))
            .map_or(self.card.x, |previous| {
                (previous.x + previous.w + rect.x) * 0.5
            });
        let right = Cell::ALL[index + 1..]
            .iter()
            .find_map(|candidate| self.cell(*candidate))
            .map_or(self.card.x + self.card.w, |next| {
                (rect.x + rect.w + next.x) * 0.5
            });
        Some(Rect::new(left, self.card.y, right - left, self.card.h))
    }
}

/// What a press on the setup screen arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupZone {
    Card { row: usize, cell: Cell },
    Start,
    PrevPage,
    NextPage,
}

/// Computes [`SetupLayout`]. `seat_choice` marks the human card; every
/// other seat receives direct difficulty and stance controls.
#[cfg(test)]
pub fn setup_layout(scenario: &Scenario, seat_choice: usize, view: Vec2, ui: f32) -> SetupLayout {
    setup_layout_page(scenario, seat_choice, view, ui, 0)
}

fn setup_layout_page(
    scenario: &Scenario,
    seat_choice: usize,
    view: Vec2,
    ui: f32,
    page: usize,
) -> SetupLayout {
    let order = seat_display_order(scenario);
    let n = order.len();
    let keys = seat_team_keys(scenario);
    let teams: Vec<u16> = {
        let mut seen = Vec::new();
        for &s in &order {
            let t = keys[s];
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen
    };
    let left_x = 56.0 * ui;
    let left_w = (view.x * 0.42).min(520.0 * ui);
    // Margins yield before content: small windows compress the title
    // zone first, then chrome, then the cards. Cards shorter than a touch
    // target switch to the paginated compact layout below.
    let top = (132.0 * ui).min(view.y * 0.22);
    let bottom = view.y - (44.0 * ui).min(view.y * 0.08);
    // Headings take rows only when a team actually groups seats; a duel
    // or an FFA would otherwise show one heading per lone seat.
    let grouped = teams.len() < n;
    let heading_rows = if grouped { teams.len() as f32 } else { 0.0 };
    let mut heading_h = 26.0 * ui;
    let mut start_h = 46.0 * ui;
    let mut gap = 6.0 * ui;
    let mut avail = bottom - top - heading_rows * heading_h - start_h - 24.0 * ui;
    let mut card_h = ((avail / n.max(1) as f32) - gap).clamp(34.0 * ui, 56.0 * ui);
    if n as f32 * (card_h + gap) > avail {
        heading_h *= 0.7;
        start_h *= 0.75;
        gap *= 0.5;
        avail = bottom - top - heading_rows * heading_h - start_h - 24.0 * ui;
        card_h = ((avail / n.max(1) as f32) - gap).max(18.0 * ui);
        // When even the ui-scaled floor overflows, the floor goes
        // physical; such cards fall below a touch target, so the compact
        // layout takes over below.
        if n as f32 * (card_h + gap) > avail {
            card_h = ((avail / n.max(1) as f32) - gap).max(14.0);
        }
    }

    if card_h < MIN_TOUCH_TARGET {
        return compact_setup_layout(scenario, seat_choice, view, ui, page);
    }

    let mut headings = Vec::new();
    let mut cards = Vec::with_capacity(n);
    let mut y = top;
    let mut last_team: Option<u16> = None;
    for &seat in &order {
        let team = keys[seat];
        if grouped && last_team != Some(team) {
            let label = format!(
                "TEAM {}",
                teams.iter().position(|t| *t == team).unwrap() + 1
            );
            headings.push((label, Rect::new(left_x, y, left_w, heading_h)));
            last_team = Some(team);
            y += heading_h;
        }
        let card = Rect::new(left_x, y, left_w, card_h);
        cards.push(Some(setup_card_controls(card, seat, seat_choice, ui)));
        y += card_h + gap;
    }
    let start = Rect::new(left_x, y + 12.0 * ui, 240.0 * ui, start_h);
    let px = left_x + left_w + 28.0 * ui;
    let preview = Rect::new(px, top, view.x - px - 40.0 * ui, bottom - top - 20.0 * ui);
    SetupLayout {
        headings,
        cards,
        start: Some(start),
        preview,
        page_prev: None,
        page_next: None,
        page: 0,
        page_count: 1,
        visible_range: [0, n + 1],
    }
}

fn setup_card_controls(card: Rect, seat: usize, seat_choice: usize, ui: f32) -> CardRects {
    // Controls occupy equal semantic lanes on the right. Their visible
    // rectangles are inset within those lanes, so even the compact 640px
    // layout keeps every mouse and touch target independent and at least
    // 44 logical pixels wide.
    let inset = (4.0 * ui).clamp(2.0, 6.0);
    let chip_h = (card.h * 0.72).clamp(10.0, 40.0 * ui);
    let chip_y = card.y + (card.h - chip_h) * 0.5;
    if seat == seat_choice {
        let controls_w = (card.w * 0.36)
            .max(MIN_TOUCH_TARGET * 2.0)
            .min(card.w - MIN_TOUCH_TARGET);
        let lane_w = controls_w / 2.0;
        let controls_x = card.x + card.w - controls_w;
        let control = |lane: usize| {
            Rect::new(
                controls_x + lane as f32 * lane_w + inset * 0.5,
                chip_y,
                (lane_w - inset).max(1.0),
                chip_h,
            )
        };
        CardRects {
            card,
            seat: Rect::new(card.x, card.y, controls_x - card.x, card.h),
            difficulty: None,
            stance: None,
            faction: control(0),
            team: control(1),
        }
    } else {
        let controls_w = (card.w * 0.72)
            .max(MIN_TOUCH_TARGET * 4.0)
            .min(card.w - MIN_TOUCH_TARGET);
        let lane_w = controls_w / 4.0;
        let controls_x = card.x + card.w - controls_w;
        let control = |lane: usize| {
            Rect::new(
                controls_x + lane as f32 * lane_w + inset * 0.5,
                chip_y,
                (lane_w - inset).max(1.0),
                chip_h,
            )
        };
        CardRects {
            card,
            seat: Rect::new(card.x, card.y, controls_x - card.x, card.h),
            difficulty: Some(control(0)),
            stance: Some(control(1)),
            faction: control(2),
            team: control(3),
        }
    }
}

fn compact_setup_layout(
    scenario: &Scenario,
    seat_choice: usize,
    view: Vec2,
    ui: f32,
    requested_page: usize,
) -> SetupLayout {
    let left_x = 56.0 * ui;
    let left_w = (view.x * 0.42).min(520.0 * ui);
    let top = (132.0 * ui).min(view.y * 0.22);
    let bottom = view.y - (44.0 * ui).min(view.y * 0.08);
    let order = seat_display_order(scenario);
    let n = order.len();
    let total_items = n + 1;
    let page_count = total_items.div_ceil(COMPACT_PAGE_ITEMS).max(1);
    let page = requested_page.min(page_count - 1);
    let first = page * COMPACT_PAGE_ITEMS;
    let past = (first + COMPACT_PAGE_ITEMS).min(total_items);
    let pager_h = MIN_TOUCH_TARGET;
    let gap = 3.0;
    let pager_gap = 4.0;
    let pager_y = bottom - pager_h;
    let content_bottom = pager_y - pager_gap;
    let item_h = ((content_bottom - top - gap * (COMPACT_PAGE_ITEMS - 1) as f32)
        / COMPACT_PAGE_ITEMS as f32)
        .max(1.0);

    let mut cards = vec![None; n];
    let mut start = None;
    for item in first..past {
        let slot = item - first;
        let rect = Rect::new(left_x, top + slot as f32 * (item_h + gap), left_w, item_h);
        if item == n {
            start = Some(Rect::new(rect.x, rect.y, (240.0 * ui).min(rect.w), rect.h));
        } else {
            cards[item] = Some(setup_card_controls(rect, order[item], seat_choice, ui));
        }
    }

    let split_gap = 6.0 * ui;
    let pager_w = (left_w - split_gap) * 0.5;
    let page_prev = (page > 0).then(|| Rect::new(left_x, pager_y, pager_w, pager_h));
    let page_next = (page + 1 < page_count)
        .then(|| Rect::new(left_x + pager_w + split_gap, pager_y, pager_w, pager_h));
    let px = left_x + left_w + 28.0 * ui;
    let preview = Rect::new(px, top, view.x - px - 40.0 * ui, bottom - top - 20.0);
    SetupLayout {
        headings: Vec::new(),
        cards,
        start,
        preview,
        page_prev,
        page_next,
        page,
        page_count,
        visible_range: [first, past],
    }
}

/// Steps the difficulty chip forward: every difficulty, then Remote, then
/// back to the first difficulty.
fn cycle_controller(plan: &mut SeatPlan) {
    if plan.remote {
        plan.remote = false;
        plan.difficulty = BotDifficulty::ALL[0];
    } else if Some(&plan.difficulty) == BotDifficulty::ALL.last() {
        plan.remote = true;
    } else {
        plan.difficulty = cycle_difficulty(plan.difficulty, 1);
    }
}

/// Whether starting the draft hosts a LAN match: a chair other than the
/// human's is remote.
pub fn draft_hosts(draft: &NewMatchDraft) -> bool {
    draft
        .seats
        .iter()
        .enumerate()
        .any(|(seat, plan)| plan.remote && seat != draft.seat_choice)
}

fn cycle_difficulty(current: BotDifficulty, direction: i8) -> BotDifficulty {
    let index = BotDifficulty::ALL
        .iter()
        .position(|candidate| *candidate == current)
        .expect("every difficulty is in ALL");
    let len = BotDifficulty::ALL.len();
    let next = if direction < 0 {
        index.checked_sub(1).unwrap_or(len - 1)
    } else {
        (index + 1) % len
    };
    BotDifficulty::ALL[next]
}

fn cycle_stance(current: BotStance, direction: i8) -> BotStance {
    let index = BotStance::ALL
        .iter()
        .position(|candidate| *candidate == current)
        .expect("every stance is in ALL");
    let len = BotStance::ALL.len();
    let next = if direction < 0 {
        index.checked_sub(1).unwrap_or(len - 1)
    } else {
        (index + 1) % len
    };
    BotStance::ALL[next]
}

/// Every Foundry anchor authored on an ASCII map: `(seat, (x, y))` for
/// each digit `1`..=`8` and letter `a`..=`h` (seats 9-16), in
/// row-major order.
pub fn seat_anchors(map: &[String]) -> Vec<(usize, (i32, i32))> {
    let mut anchors = Vec::new();
    for (y, row) in map.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let seat = match ch {
                '1'..='8' => Some(ch as usize - '1' as usize),
                'a'..='h' => Some(8 + ch as usize - 'a' as usize),
                _ => None,
            };
            if let Some(seat) = seat {
                anchors.push((seat, (x.fit::<i32>(), y.fit::<i32>())));
            }
        }
    }
    anchors
}

/// Marks every seat's foundry on a drawn preview rect: numbered discs
/// in the seat's effective faction color (chip overrides included), a
/// white ring for the human's chair, an accent ring for the focused
/// seat.
pub fn draw_seat_markers(
    scenario: &Scenario,
    draft: &NewMatchDraft,
    rect: Rect,
    seat_choice: usize,
    focus_seat: Option<usize>,
    ui: f32,
) {
    let map_w = scenario.map.first().map_or(1, |r| r.chars().count()) as f32;
    let map_h = scenario.map.len() as f32;
    for (seat, (ax, ay)) in seat_anchors(&scenario.map) {
        if scenario.players.get(seat).is_none() {
            continue;
        }
        // Foundry anchors are the 2x2's top-left; mark its center.
        let px = rect.x + (ax as f32 + 1.0) / map_w * rect.w;
        let py = rect.y + (ay as f32 + 1.0) / map_h * rect.h;
        let accent = crate::render::faction_accent(effective_faction(scenario, draft, seat));
        if seat == seat_choice {
            draw_circle_lines(px, py, 10.0 * ui, 2.5, macroquad::prelude::WHITE);
        } else if focus_seat == Some(seat) {
            draw_circle_lines(px, py, 10.0 * ui, 2.0, accent);
        }
        draw_circle(px, py, 7.5 * ui, accent);
        let label = format!("{}", seat + 1);
        let tw = measure_text(&label, None, numeric::font_size(13.0 * ui), 1.0).width;
        draw_text(
            &label,
            px - tw * 0.5,
            py + 4.5 * ui,
            13.0 * ui,
            Color::from_rgba(20, 20, 24, 255),
        );
    }
}

impl Wizard {
    /// Opens at the map grid, the remembered map re-highlighted.
    pub fn open(draft: &NewMatchDraft) -> Self {
        let entries = discover_scenarios();
        let mut browser = Browser::new();
        if draft.scenario.is_some() {
            browser.select_path(&entries, draft.scenario_path.as_deref());
        }
        Self {
            step: Step::Map,
            entries,
            browser,
            setup_sel: 0,
            setup_cell: Cell::Seat,
            setup_press: Press::default(),
            back: crate::button::BackButton::default(),
            setup_page: 0,
        }
    }

    /// The entry for the draft's picked map, with its stable index —
    /// what the setup screen keys its preview by.
    pub fn picked_entry(&self, draft: &NewMatchDraft) -> Option<(usize, &ScenarioEntry)> {
        self.entries
            .iter()
            .position(|e| e.path == draft.scenario_path)
            .and_then(|i| self.entries.get(i).map(|e| (i, e)))
    }

    fn goto(&mut self, step: Step, draft: &NewMatchDraft) {
        self.step = step;
        self.back.cancel();
        match step {
            Step::Map => {
                self.entries = discover_scenarios();
                self.browser
                    .select_path(&self.entries, draft.scenario_path.as_deref());
            }
            Step::Setup => {
                // Start preselected: Enter-Enter from the grid plays
                // the map as authored.
                self.setup_sel = draft.seats.len();
                self.setup_page = self.setup_sel / COMPACT_PAGE_ITEMS;
                self.setup_cell = Cell::Seat;
                self.setup_press.cancel();
            }
        }
    }

    /// Applies a frame's events. Windowless: the screens navigate, the
    /// draft records answers, and the return says whether the caller
    /// should stay, go Home, or launch the match.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        draft: &mut NewMatchDraft,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Result<Out> {
        // The corner Back button sees the pointer first; the step only
        // gets the events it leaves alone.
        let (back, events) = self.back.route(events);
        if back {
            sounds.push((SoundKind::Click, None));
            match self.step {
                Step::Map => return Ok(Out::Home),
                Step::Setup => {
                    self.goto(Step::Map, draft);
                    return Ok(Out::Stay);
                }
            }
        }
        match self.step {
            Step::Map => match self.browser.handle(&self.entries, &events, mouse) {
                BrowserOut::Back => return Ok(Out::Home),
                BrowserOut::Pick(entry) => {
                    sounds.push((SoundKind::Click, None));
                    let scenario = match &self.entries[entry].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    draft.set_scenario(scenario, self.entries[entry].path.clone());
                    self.goto(Step::Setup, draft);
                }
                BrowserOut::Stay => {}
            },
            Step::Setup => {
                if let Some(out) = self.update_setup(&events, mouse, draft, sounds) {
                    return Ok(out);
                }
            }
        }
        Ok(Out::Stay)
    }

    /// The setup screen's input: Up/Down walk the seat cards and the
    /// Start button; Left/Right walk the seat, difficulty, stance,
    /// faction, and team cells; Enter takes the seat or cycles the
    /// control under the cursor; clicks hit each zone directly.
    fn update_setup(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        draft: &mut NewMatchDraft,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Option<Out> {
        let Some(scenario) = draft.scenario.as_deref() else {
            self.goto(Step::Map, draft);
            return None;
        };
        let order = seat_display_order(scenario);
        let start_index = order.len();
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        self.setup_page = self.setup_sel.min(start_index) / COMPACT_PAGE_ITEMS;
        let layout = setup_layout_page(scenario, draft.seat_choice, view, ui, self.setup_page);
        self.setup_page = layout.page;
        let cell_live = |row: usize, cell: Cell| -> bool {
            row < start_index
                && match cell {
                    Cell::Seat | Cell::Faction | Cell::Team => true,
                    Cell::Difficulty => order[row] != draft.seat_choice,
                    Cell::Stance => {
                        order[row] != draft.seat_choice && !draft.seats[order[row]].remote
                    }
                }
        };
        let zone_at = |p: Vec2, touch: bool| -> Option<SetupZone> {
            for (row, card) in layout.cards.iter().enumerate() {
                let Some(card) = card else {
                    continue;
                };
                for cell in Cell::ALL.into_iter().filter(|cell| cell_live(row, *cell)) {
                    let rect = if touch {
                        card.touch_cell(cell)
                    } else {
                        card.cell(cell)
                    };
                    if rect.is_some_and(|rect| rect.contains(p)) {
                        return Some(SetupZone::Card { row, cell });
                    }
                }
            }
            if layout.start.is_some_and(|rect| rect.contains(p)) {
                return Some(SetupZone::Start);
            }
            if layout.page_prev.is_some_and(|rect| rect.contains(p)) {
                return Some(SetupZone::PrevPage);
            }
            layout
                .page_next
                .is_some_and(|rect| rect.contains(p))
                .then_some(SetupZone::NextPage)
        };
        let mut activate: Option<SetupZone> = None;
        for event in events {
            if let Fed::Activated(zone) = self.setup_press.feed(event, zone_at) {
                if let Some(p) = crate::press::position(event) {
                    *mouse = p;
                }
                match zone {
                    SetupZone::Card { row, cell } => {
                        self.setup_sel = row;
                        if cell_live(row, cell) {
                            self.setup_cell = cell;
                        }
                    }
                    SetupZone::Start => self.setup_sel = start_index,
                    SetupZone::PrevPage | SetupZone::NextPage => {}
                }
                activate = Some(zone);
                break;
            }
            match *event {
                RawEvent::KeyDown { key: Key::Escape } => {
                    self.goto(Step::Map, draft);
                    return None;
                }
                RawEvent::KeyDown { key: Key::Enter } => {
                    activate = Some(if self.setup_sel >= start_index {
                        SetupZone::Start
                    } else {
                        // The sticky column falls back to the seat zone
                        // on rows where its cell is dead.
                        let cell = if cell_live(self.setup_sel, self.setup_cell) {
                            self.setup_cell
                        } else {
                            Cell::Seat
                        };
                        SetupZone::Card {
                            row: self.setup_sel,
                            cell,
                        }
                    });
                    break;
                }
                RawEvent::KeyDown { .. } => {
                    let Some(nav) = Nav::decode(event) else {
                        continue;
                    };
                    let rows = start_index + 1;
                    if let Some(row) = step_line(
                        rows,
                        self.setup_sel,
                        nav,
                        Axis::Vertical,
                        COMPACT_PAGE_ITEMS,
                        |_| true,
                    ) {
                        self.setup_sel = row;
                        self.setup_page = row / COMPACT_PAGE_ITEMS;
                    } else if let Some(cell) = step_line(
                        Cell::ALL.len(),
                        self.setup_cell.index(),
                        nav,
                        Axis::Horizontal,
                        1,
                        |cell| cell_live(self.setup_sel, Cell::ALL[cell]),
                    ) {
                        self.setup_cell = Cell::ALL[cell];
                    }
                }
                RawEvent::MouseMove { x, y } => *mouse = vec2(x, y),
                RawEvent::TouchDown { id, x, y } | RawEvent::TouchMove { id, x, y }
                    if self.setup_press.owns(id) =>
                {
                    *mouse = vec2(x, y);
                }
                _ => {}
            }
        }
        let zone = activate?;
        self.setup_press.cancel();
        match zone {
            SetupZone::PrevPage => {
                self.setup_page = self.setup_page.saturating_sub(1);
                self.setup_sel = self.setup_page * COMPACT_PAGE_ITEMS;
                self.setup_cell = Cell::Seat;
                sounds.push((SoundKind::Click, None));
            }
            SetupZone::NextPage => {
                self.setup_page = (self.setup_page + 1).min(layout.page_count - 1);
                self.setup_sel = (self.setup_page * COMPACT_PAGE_ITEMS).min(start_index);
                self.setup_cell = Cell::Seat;
                sounds.push((SoundKind::Click, None));
            }
            SetupZone::Start => {
                // Refuse an all-one-team draft here so the reason shows
                // inline instead of a failed-launch notice.
                if draft_one_team(draft) {
                    sounds.push((SoundKind::Denied, None));
                    return None;
                }
                sounds.push((SoundKind::Click, None));
                return Some(Out::Launch);
            }
            SetupZone::Card { row, cell } => {
                sounds.push((SoundKind::Click, None));
                let seat = order[row];
                let plan = &mut draft.seats[seat];
                match cell {
                    // Seat choice never permutes seats or their other
                    // choices; it moves the human's chair.
                    Cell::Seat => draft.seat_choice = seat,
                    Cell::Difficulty => cycle_controller(plan),
                    Cell::Stance => plan.stance = cycle_stance(plan.stance, 1),
                    Cell::Faction => {
                        plan.faction_choice = (plan.faction_choice + 1) % FACTION_CHIP_ITEMS.len();
                    }
                    // FFA, then every team up to the seat count
                    // (start_index is the full roster's length),
                    // wrapping back to FFA.
                    Cell::Team => plan.team_choice = (plan.team_choice + 1) % (start_index + 1),
                }
            }
        }
        None
    }

    /// Draws the setup screen: team-grouped seat cards, the Start
    /// button, and the live map with every chair marked.
    #[expect(
        clippy::too_many_lines,
        reason = "lays out and draws the whole setup screen"
    )]
    pub fn draw_setup(&self, draft: &NewMatchDraft, previews: &mut PreviewCache) {
        let Some(scenario) = draft.scenario.as_deref() else {
            return;
        };
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let layout = setup_layout_page(scenario, draft.seat_choice, view, ui, self.setup_page);
        let order = seat_display_order(scenario);

        let title = "MATCH SETUP";
        let compact = layout.page_count > 1;
        let tsize = if compact {
            (56.0 * ui).min(42.0)
        } else {
            56.0 * ui
        };
        let tdims = measure_text(title, None, numeric::font_size(tsize), 1.0);
        draw_text(
            title,
            (view.x - tdims.width) * 0.5,
            if compact { 58.0 } else { 64.0 * ui },
            tsize,
            TEXT_TITLE,
        );
        if !compact {
            let sub = if crate::hints::showing() {
                format!(
                    "{} - pick your seat, opponents, factions, and teams",
                    scenario.name
                )
            } else {
                scenario.name.clone()
            };
            let sdims = measure_text(&sub, None, numeric::font_size(18.0 * ui), 1.0);
            draw_text(
                &sub,
                (view.x - sdims.width) * 0.5,
                92.0 * ui,
                18.0 * ui,
                TEXT_SECONDARY,
            );
        }

        for (label, rect) in &layout.headings {
            draw_text(label, rect.x, rect.y + rect.h * 0.7, 17.0 * ui, TEXT_TITLE);
            let dims = measure_text(label, None, numeric::font_size(17.0 * ui), 1.0);
            draw_rectangle(
                rect.x + dims.width + 12.0 * ui,
                rect.y + rect.h * 0.55,
                rect.w - dims.width - 12.0 * ui,
                1.0,
                Color::new(0.6, 0.6, 0.65, 0.25),
            );
        }
        for (pos, card) in layout.cards.iter().enumerate() {
            let Some(card) = card else {
                continue;
            };
            let rect = &card.card;
            let seat = order[pos];
            let display = effective_name(scenario, draft, seat);
            let selected = self.setup_sel == pos;
            let is_you = seat == draft.seat_choice;
            let plan = draft.seats[seat];
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, SURFACE_MENU);
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if selected { 2.5 } else { 1.0 },
                if selected {
                    TEXT_TITLE
                } else {
                    Color::new(0.6, 0.6, 0.65, 0.3)
                },
            );
            let accent = crate::render::faction_accent(effective_faction(scenario, draft, seat));
            let cy = rect.y + rect.h * 0.5;
            let chip_x = rect.x + 22.0 * ui;
            // Everything on a card scales to the card, so a compressed
            // roster never draws across its neighbors and chips.
            let disc = (10.0 * ui).min(rect.h * 0.38);
            if is_you {
                draw_circle_lines(chip_x, cy, disc * 1.3, 2.0, macroquad::prelude::WHITE);
            }
            draw_circle(chip_x, cy, disc, accent);
            let num = format!("{}", seat + 1);
            let num_font = (14.0 * ui).min(rect.h * 0.55);
            let ndims = measure_text(&num, None, numeric::font_size(num_font), 1.0);
            draw_text(
                &num,
                chip_x - ndims.width * 0.5,
                cy + num_font * 0.35,
                num_font,
                Color::from_rgba(20, 20, 24, 255),
            );
            let mut name_font = (16.0 * ui).min(rect.h * 0.62);
            let text_right = card
                .difficulty
                .map_or(card.seat.x + card.seat.w, |difficulty| difficulty.x);
            let name_room = (text_right - rect.x - 48.0 * ui).max(20.0);
            let nw = measure_text(&display, None, numeric::font_size(name_font), 1.0).width;
            if nw > name_room {
                name_font = (name_font * name_room / nw).max(8.0);
            }
            draw_text(
                &display,
                rect.x + 44.0 * ui,
                cy + name_font * 0.35,
                name_font,
                TEXT_PRIMARY,
            );
            if is_you {
                let tag = "your seat";
                let tag_font = (14.0 * ui).min(rect.h * 0.55);
                let tdims = measure_text(tag, None, numeric::font_size(tag_font), 1.0);
                let fac = card.faction;
                draw_text(
                    tag,
                    fac.x - tdims.width - 14.0 * ui,
                    cy + tag_font * 0.35,
                    tag_font,
                    TEXT_SECONDARY,
                );
            }
            let bot_labels = if plan.remote {
                ["Remote", ""]
            } else {
                [difficulty_name(plan.difficulty), stance_name(plan.stance)]
            };
            for (cell, control, label) in [
                (Cell::Difficulty, card.difficulty, bot_labels[0]),
                (Cell::Stance, card.stance, bot_labels[1]),
            ] {
                let Some(control) = control.filter(|_| !label.is_empty()) else {
                    continue;
                };
                let on_cell = selected && self.setup_cell == cell;
                draw_rectangle(
                    control.x,
                    control.y,
                    control.w,
                    control.h,
                    Color::from_rgba(27, 37, 39, 255),
                );
                draw_rectangle_lines(
                    control.x,
                    control.y,
                    control.w,
                    control.h,
                    if on_cell { 2.0 } else { 1.0 },
                    if on_cell { TEXT_TITLE } else { accent },
                );
                let mut font = 13.0 * ui;
                let mut dims = measure_text(label, None, numeric::font_size(font), 1.0);
                if dims.width > control.w - 6.0 {
                    font = (font * (control.w - 6.0) / dims.width).max(8.0);
                    dims = measure_text(label, None, numeric::font_size(font), 1.0);
                }
                draw_text(
                    label,
                    control.x + (control.w - dims.width) * 0.5,
                    control.y + control.h * 0.5 + font * 0.35,
                    font,
                    accent,
                );
            }
            // Boxed editable chips; the cursor's cell wears the accent.
            let team_label = team_chip_label(plan.team_choice);
            for (cell, chip, label) in [
                (
                    Cell::Faction,
                    card.faction,
                    FACTION_CHIP_ITEMS[plan.faction_choice],
                ),
                (Cell::Team, card.team, team_label.as_str()),
            ] {
                let on_cell = selected && self.setup_cell == cell;
                draw_rectangle(
                    chip.x,
                    chip.y,
                    chip.w,
                    chip.h,
                    Color::from_rgba(32, 32, 38, 255),
                );
                draw_rectangle_lines(
                    chip.x,
                    chip.y,
                    chip.w,
                    chip.h,
                    if on_cell { 2.0 } else { 1.0 },
                    if on_cell {
                        TEXT_TITLE
                    } else {
                        Color::new(0.6, 0.6, 0.65, 0.35)
                    },
                );
                // The label fits its chip: squeezed cards shrink the type
                // instead of spilling text across neighbors.
                let mut font = 13.0 * ui;
                let mut ldims = measure_text(label, None, numeric::font_size(font), 1.0);
                if ldims.width > chip.w - 6.0 {
                    font = (font * (chip.w - 6.0) / ldims.width).max(8.0);
                    ldims = measure_text(label, None, numeric::font_size(font), 1.0);
                }
                draw_text(
                    label,
                    chip.x + (chip.w - ldims.width) * 0.5,
                    chip.y + chip.h * 0.5 + font * 0.35,
                    font,
                    if on_cell {
                        TEXT_PRIMARY
                    } else {
                        TEXT_SECONDARY
                    },
                );
            }
            // The seat-zone cell cursor: a soft inner line under
            // the name, so "Enter takes this chair" reads.
            if selected && self.setup_cell == Cell::Seat && !is_you {
                let zone = card.seat;
                draw_rectangle(
                    zone.x + 44.0 * ui,
                    cy + name_font * 0.55,
                    measure_text(&display, None, numeric::font_size(name_font), 1.0).width,
                    1.5,
                    TEXT_TITLE,
                );
            }
        }
        // Start button, disabled for an all-one-team draft.
        let one_team = draft_one_team(draft);
        let start_selected = self.setup_sel == layout.cards.len();
        if let Some(start) = layout.start {
            draw_rectangle(start.x, start.y, start.w, start.h, SURFACE_MENU);
            draw_rectangle_lines(
                start.x,
                start.y,
                start.w,
                start.h,
                if start_selected { 3.0 } else { 1.5 },
                if one_team {
                    Color::new(0.6, 0.6, 0.65, 0.4)
                } else if start_selected {
                    TEXT_TITLE
                } else {
                    TEXT_SECONDARY
                },
            );
            let label = start_label(draft);
            let ldims = measure_text(label, None, numeric::font_size(20.0 * ui), 1.0);
            draw_text(
                label,
                start.x + (start.w - ldims.width) * 0.5,
                start.y + start.h * 0.66,
                20.0 * ui,
                if !one_team && start_selected {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                },
            );
        }

        for (rect, label) in [
            (
                layout.page_prev,
                format!("PREV  {}/{}", layout.page + 1, layout.page_count),
            ),
            (
                layout.page_next,
                format!("{}/{}  NEXT", layout.page + 1, layout.page_count),
            ),
        ] {
            let Some(rect) = rect else {
                continue;
            };
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, SURFACE_MENU);
            draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.5, TEXT_SECONDARY);
            let mut size = 16.0 * ui;
            let mut dims = measure_text(&label, None, numeric::font_size(size), 1.0);
            if dims.width > rect.w - 8.0 {
                size = (size * (rect.w - 8.0) / dims.width).max(8.0);
                dims = measure_text(&label, None, numeric::font_size(size), 1.0);
            }
            draw_text(
                &label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h * 0.5 + size * 0.35,
                size,
                TEXT_SECONDARY,
            );
        }

        if let Some((_, entry)) = self.picked_entry(draft)
            && let Some(tex) = previews.get(entry)
        {
            let scale = (layout.preview.w / tex.width()).min(layout.preview.h / tex.height());
            let (pw, ph) = (tex.width() * scale, tex.height() * scale);
            let x = layout.preview.x + (layout.preview.w - pw) * 0.5;
            let y = layout.preview.y + (layout.preview.h - ph) * 0.5;
            draw_rectangle(
                x - 8.0 * ui,
                y - 8.0 * ui,
                pw + 16.0 * ui,
                ph + 16.0 * ui,
                SURFACE_MENU,
            );
            draw_texture_ex(
                tex,
                x,
                y,
                crate::render::theme_tint(&entry.theme),
                DrawTextureParams {
                    dest_size: Some(vec2(pw, ph)),
                    ..Default::default()
                },
            );
            let focus = (self.setup_sel < order.len()).then(|| order[self.setup_sel]);
            draw_seat_markers(
                scenario,
                draft,
                Rect::new(x, y, pw, ph),
                draft.seat_choice,
                focus,
                ui,
            );
        }

        if !compact {
            let hint = setup_hint(
                one_team,
                self.setup_sel == order.len(),
                self.setup_cell,
                crate::platform::TOUCH_ONLY,
            );
            let hint = crate::menu::binding_hint(hint);
            let hdims = measure_text(&hint, None, numeric::font_size(16.0 * ui), 1.0);
            draw_text(
                &hint,
                (view.x - hdims.width) * 0.5,
                view.y - 20.0 * ui,
                16.0 * ui,
                // The one-team warning is information, not coaching.
                if one_team {
                    TEXT_DANGER
                } else {
                    crate::hints::fade(TEXT_SECONDARY)
                },
            );
        }
    }

    /// The debug protocol's stable mode name for the current step, which
    /// automation scripts depend on.
    pub fn mode_name(&self) -> &'static str {
        match self.step {
            Step::Map => "main_menu",
            Step::Setup => "match_setup",
        }
    }

    /// The (title, items, selected) surface `QueryUi` reports — the
    /// custom screens speak the same protocol the row menus do.
    pub fn ui_surface(&self, draft: &NewMatchDraft) -> (String, Vec<String>, usize) {
        match self.step {
            Step::Map => (
                "OXIDE".to_string(),
                self.entries.iter().map(|e| e.label.clone()).collect(),
                self.browser.selected,
            ),
            Step::Setup => {
                let mut items: Vec<String> = draft
                    .scenario
                    .as_deref()
                    .map(|sc| {
                        seat_display_order(sc)
                            .into_iter()
                            .map(|seat| {
                                let name = effective_name(sc, draft, seat);
                                let plan = draft.seats[seat];
                                let team = team_chip_label(plan.team_choice);
                                if seat == draft.seat_choice || plan.remote {
                                    format!(
                                        "{}. {} ({}) | {} | {}",
                                        seat + 1,
                                        name,
                                        if seat == draft.seat_choice {
                                            "you"
                                        } else {
                                            "remote"
                                        },
                                        FACTION_CHIP_ITEMS[plan.faction_choice],
                                        team
                                    )
                                } else {
                                    format!(
                                        "{}. {} | Difficulty {} | Stance {} | {} | {}",
                                        seat + 1,
                                        name,
                                        difficulty_name(plan.difficulty),
                                        stance_name(plan.stance),
                                        FACTION_CHIP_ITEMS[plan.faction_choice],
                                        team
                                    )
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                items.push(start_label(draft).to_string());
                ("MATCH SETUP".to_string(), items, self.setup_sel)
            }
        }
    }

    /// The half-open item-index range actually on screen — `QueryUi`'s
    /// `visible_range`, computed from the same injected viewport the
    /// frame drew with. The map grid reads the browser's real layout
    /// (visible cards are a contiguous run of entry indices; a window
    /// showing none reports `[0, 0]`). Compact setup reports only the
    /// current nonoverlapping page; full-height setup reports every row.
    pub fn ui_visible_range(&self, draft: &NewMatchDraft, view: Vec2, ui: f32) -> [usize; 2] {
        match self.step {
            Step::Map => {
                let layout = self.browser.layout(&self.entries, view, ui);
                match (layout.cards.first(), layout.cards.last()) {
                    (Some(&(first, _)), Some(&(last, _))) => [first, last + 1],
                    _ => [0, 0],
                }
            }
            Step::Setup => draft.scenario.as_deref().map_or([0, 0], |scenario| {
                setup_layout_page(scenario, draft.seat_choice, view, ui, self.setup_page)
                    .visible_range
            }),
        }
    }

    /// The pointer's current highlight for the protocol surface: the
    /// grid's hovered card on the map step (the UX battery's row
    /// discovery sweeps this), nothing on setup.
    pub fn ui_hover(&self) -> Option<usize> {
        match self.step {
            Step::Map => self.browser.hover,
            Step::Setup => None,
        }
    }
}

pub(crate) mod launch;

#[cfg(test)]
mod tests;
