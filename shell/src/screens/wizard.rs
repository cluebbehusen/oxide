//! The New Match flow: the map browser grid, then the match setup
//! screen — seat cards grouped by team beside a large who-is-where
//! preview — for every map size. Duels get the same seat, faction,
//! and team choices the larger maps do (the old 1v1
//! quick-question flow could not arrange a mirror match or a seat
//! swap, and Enter-Enter still launches the classic matchup).
//!
//! One back end: every answer lands in the draft's PER-SEAT vector,
//! and `launch()` reads only that.

use crate::bot_label::{difficulty_name, stance_name};
use crate::game::SoundKind;
use crate::menu::{PreviewCache, ScenarioEntry, discover_scenarios};
use crate::screens::browser::{Browser, Out as BrowserOut};
use anyhow::{Context, Result};
use macroquad::prelude::{
    Color, DrawTextureParams, Rect, Vec2, draw_circle, draw_circle_lines, draw_rectangle,
    draw_rectangle_lines, draw_text, draw_texture_ex, measure_text, vec2,
};
use oxide_protocol::{Key, MouseButton, RawEvent};
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
    choice.checked_sub(1).map(|team| team as u8)
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
/// card's disc and its label can't drift apart again. (Duplicate-name
/// ordinals are launch's business; the preview shows the pre-ordinal
/// name.)
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
    /// The picked map's path (`None` = the embedded skirmish) — keyed
    /// by PATH so the browser's section sort can never move the
    /// remembered highlight onto a different map.
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
    /// Installs a picked map. Re-entering the SAME map keeps every
    /// earlier answer (the draft survives Back, by doctrine); a
    /// different map resets the seats and the chair — a seat 5 taken
    /// on an 8-seat map once silently carried into a duel as "the
    /// second chair", with nothing on screen saying so.
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
    pub setup_cell: usize,
    /// Setup zone armed by a press: (row, cell); activation on
    /// release inside the same zone. Rows after Start are compact-page
    /// navigation controls.
    setup_pressed: Option<(usize, usize)>,
    /// Finger and setup zone armed by a touch. Other fingers are ignored
    /// until the owner releases, matching the shared menu gesture contract.
    setup_pressed_touch: Option<(u64, usize, usize)>,
    /// Compact setup page. Full-height layouts always clamp this to zero.
    setup_page: usize,
}

/// One collision-free team key per seat: an authored id stays
/// itself; an omitted seat surrogates as its own index lifted above
/// the whole u8 range, so no authored id can alias it and every pass
/// that groups seats derives the identical key. (The old scheme keyed
/// omitted seats two different ways — `teams.len()` in one pass, the
/// seat index in the next — and a mixed-team map could drop a seat
/// from the setup order entirely.)
fn seat_team_keys(scenario: &Scenario) -> Vec<u16> {
    scenario
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| p.team.map(u16::from).unwrap_or(256 + i as u16))
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
    /// One card rect per DISPLAY position (see [`seat_display_order`]).
    pub seats: Vec<Rect>,
    /// Per card, the three interactive zones: the seat itself, its
    /// faction chip, and its team chip.
    pub cells: Vec<[Rect; 3]>,
    /// Per card, the directly editable difficulty and stance controls.
    /// Both rectangles are empty on the human's card.
    pub bot_controls: Vec<[Rect; 2]>,
    /// The Start button.
    pub start: Rect,
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
    // zone first, then chrome, then the cards — the full roster and
    // the Start button stay on screen by construction (the old fixed
    // 34ui card floor pushed Start past the bottom of a 640x400
    // window on eight-seat maps, with no scrolling to reach it).
    let top = (132.0 * ui).min(view.y * 0.22);
    let bottom = view.y - (44.0 * ui).min(view.y * 0.08);
    // Headings earn their rows only when a team actually groups
    // seats: a duel (or an FFA) under "TEAM 1 / TEAM 2 / ..." — one
    // singleton per label — was noise wearing a uniform.
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
        // A large UI scale cannot conjure height: when the ui-scaled
        // floor still overflows, the floor goes PHYSICAL — controls
        // run small, but the whole roster and Start stay on screen
        // and clickable at every supported window.
        if n as f32 * (card_h + gap) > avail {
            card_h = ((avail / n.max(1) as f32) - gap).max(14.0);
        }
    }

    if card_h < MIN_TOUCH_TARGET {
        return compact_setup_layout(scenario, seat_choice, view, ui, page);
    }

    let mut headings = Vec::new();
    let mut seats = vec![Rect::new(0.0, 0.0, 0.0, 0.0); n];
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cells = vec![[zero; 3]; n];
    let mut bot_controls = vec![[zero; 2]; n];
    let mut y = top;
    let mut last_team: Option<u16> = None;
    for (pos, &seat) in order.iter().enumerate() {
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
        seats[pos] = card;
        let (card_cells, controls) = setup_card_controls(card, seat, seat_choice, ui);
        cells[pos] = card_cells;
        bot_controls[pos] = controls;
        y += card_h + gap;
    }
    let start = Rect::new(left_x, y + 12.0 * ui, 240.0 * ui, start_h);
    let px = left_x + left_w + 28.0 * ui;
    let preview = Rect::new(px, top, view.x - px - 40.0 * ui, bottom - top - 20.0 * ui);
    SetupLayout {
        headings,
        seats,
        cells,
        bot_controls,
        start,
        preview,
        page_prev: None,
        page_next: None,
        page: 0,
        page_count: 1,
        visible_range: [0, n + 1],
    }
}

fn setup_card_controls(
    card: Rect,
    seat: usize,
    seat_choice: usize,
    ui: f32,
) -> ([Rect; 3], [Rect; 2]) {
    // Controls occupy equal semantic lanes on the right. Their visible
    // rectangles are inset within those lanes, so even the compact 640px
    // layout keeps every mouse and touch target independent and at least
    // 44 logical pixels wide.
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let inset = (4.0 * ui).clamp(2.0, 6.0);
    let chip_h = (card.h * 0.72).clamp(10.0, 40.0 * ui);
    let chip_y = card.y + (card.h - chip_h) * 0.5;
    if seat != seat_choice {
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
        let difficulty = control(0);
        let stance = control(1);
        let faction = control(2);
        let team = control(3);
        (
            [
                Rect::new(card.x, card.y, controls_x - card.x, card.h),
                faction,
                team,
            ],
            [difficulty, stance],
        )
    } else {
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
        let faction = control(0);
        let team = control(1);
        (
            [
                Rect::new(card.x, card.y, controls_x - card.x, card.h),
                faction,
                team,
            ],
            [zero; 2],
        )
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

    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut seats = vec![zero; n];
    let mut cells = vec![[zero; 3]; n];
    let mut bot_controls = vec![[zero; 2]; n];
    let mut start = zero;
    for item in first..past {
        let slot = item - first;
        let rect = Rect::new(left_x, top + slot as f32 * (item_h + gap), left_w, item_h);
        if item == n {
            start = Rect::new(rect.x, rect.y, (240.0 * ui).min(rect.w), rect.h);
        } else {
            seats[item] = rect;
            let seat = order[item];
            let (card_cells, controls) = setup_card_controls(rect, seat, seat_choice, ui);
            cells[item] = card_cells;
            bot_controls[item] = controls;
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
        seats,
        cells,
        bot_controls,
        start,
        preview,
        page_prev,
        page_next,
        page,
        page_count,
        visible_range: [first, past],
    }
}

/// Returns one setup control in left-to-right keyboard order.
fn setup_cell_rect(layout: &SetupLayout, row: usize, cell: usize) -> Option<Rect> {
    let rect = match cell {
        0 => *layout.cells.get(row)?.first()?,
        1 => *layout.bot_controls.get(row)?.first()?,
        2 => *layout.bot_controls.get(row)?.get(1)?,
        3 => *layout.cells.get(row)?.get(1)?,
        4 => *layout.cells.get(row)?.get(2)?,
        _ => return None,
    };
    (rect.w > 0.0).then_some(rect)
}

fn setup_touch_cell_rect(layout: &SetupLayout, row: usize, cell: usize) -> Option<Rect> {
    let card = *layout.seats.get(row)?;
    let rect = setup_cell_rect(layout, row, cell)?;
    let left = (0..cell)
        .rev()
        .find_map(|candidate| setup_cell_rect(layout, row, candidate))
        .map_or(card.x, |previous| (previous.x + previous.w + rect.x) * 0.5);
    let right = (cell + 1..5)
        .find_map(|candidate| setup_cell_rect(layout, row, candidate))
        .map_or(card.x + card.w, |next| (rect.x + rect.w + next.x) * 0.5);
    Some(Rect::new(left, card.y, right - left, card.h))
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
                anchors.push((seat, (x as i32, y as i32)));
            }
        }
    }
    anchors
}

/// Marks every seat's foundry on a drawn preview rect: numbered discs
/// in the seat's EFFECTIVE faction color (chip overrides included), a
/// white ring for the human's chair, an accent ring for the focused
/// seat — the list and the map read as one thing.
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
        let tw = measure_text(&label, None, (13.0 * ui) as u16, 1.0).width;
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
            browser.select_path(&entries, &draft.scenario_path);
        }
        Self {
            step: Step::Map,
            entries,
            browser,
            setup_sel: 0,
            setup_cell: 0,
            setup_pressed: None,
            setup_pressed_touch: None,
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
        match step {
            Step::Map => {
                self.entries = discover_scenarios();
                self.browser
                    .select_path(&self.entries, &draft.scenario_path);
            }
            Step::Setup => {
                // Start preselected: Enter-Enter from the grid plays
                // the map as authored — the classic launch is still
                // two keypresses on every map size.
                self.setup_sel = draft.seats.len();
                self.setup_page = self.setup_sel / COMPACT_PAGE_ITEMS;
                self.setup_cell = 0;
                self.setup_pressed = None;
                self.setup_pressed_touch = None;
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
        match self.step {
            Step::Map => match self.browser.handle(&self.entries, events, mouse) {
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
                if let Some(out) = self.update_setup(events, mouse, draft, sounds) {
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
        let previous_page_index = start_index + 1;
        let next_page_index = start_index + 2;
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        self.setup_page = self.setup_sel.min(start_index) / COMPACT_PAGE_ITEMS;
        let layout = setup_layout_page(scenario, draft.seat_choice, view, ui, self.setup_page);
        self.setup_page = layout.page;
        let cell_live = |row: usize, cell: usize| -> bool {
            row < start_index
                && match cell {
                    0 | 3 | 4 => true,
                    1 | 2 => order[row] != draft.seat_choice,
                    _ => false,
                }
        };
        let zone_at = |p: Vec2, touch: bool| -> Option<(usize, usize)> {
            for row in 0..layout.cells.len() {
                for cell in 0..5 {
                    let rect = if touch {
                        setup_touch_cell_rect(&layout, row, cell)
                    } else {
                        setup_cell_rect(&layout, row, cell)
                    };
                    if rect.is_some_and(|rect| rect.contains(p)) {
                        return Some((row, cell));
                    }
                }
            }
            if layout.start.w > 0.0 && layout.start.contains(p) {
                return Some((start_index, 0));
            }
            if layout.page_prev.is_some_and(|rect| rect.contains(p)) {
                return Some((previous_page_index, 0));
            }
            layout
                .page_next
                .is_some_and(|rect| rect.contains(p))
                .then_some((next_page_index, 0))
        };
        let mut activate: Option<(usize, usize)> = None;
        for event in events {
            match *event {
                RawEvent::KeyDown { key: Key::Escape } => {
                    self.goto(Step::Map, draft);
                    return None;
                }
                RawEvent::KeyDown { key: Key::Up } => {
                    self.setup_sel = self.setup_sel.checked_sub(1).unwrap_or(start_index);
                    self.setup_page = self.setup_sel / COMPACT_PAGE_ITEMS;
                }
                RawEvent::KeyDown { key: Key::Down } => {
                    self.setup_sel = (self.setup_sel + 1) % (start_index + 1);
                    self.setup_page = self.setup_sel / COMPACT_PAGE_ITEMS;
                }
                RawEvent::KeyDown { key: Key::Left } => {
                    let mut c = self.setup_cell;
                    while c > 0 {
                        c -= 1;
                        if cell_live(self.setup_sel, c) {
                            break;
                        }
                    }
                    self.setup_cell = c;
                }
                RawEvent::KeyDown { key: Key::Right } => {
                    let mut c = self.setup_cell + 1;
                    while c <= 4 && !cell_live(self.setup_sel, c) {
                        c += 1;
                    }
                    if c <= 4 && cell_live(self.setup_sel, c) {
                        self.setup_cell = c;
                    }
                }
                RawEvent::KeyDown { key: Key::Home } => {
                    self.setup_sel = 0;
                    self.setup_page = 0;
                }
                RawEvent::KeyDown { key: Key::End } => {
                    self.setup_sel = start_index;
                    self.setup_page = start_index / COMPACT_PAGE_ITEMS;
                }
                RawEvent::KeyDown { key: Key::Enter } => {
                    // The sticky column falls back to the seat zone on
                    // rows where its cell is dead.
                    let cell = if cell_live(self.setup_sel, self.setup_cell) {
                        self.setup_cell
                    } else {
                        0
                    };
                    activate = Some((self.setup_sel, cell));
                    break;
                }
                RawEvent::MouseMove { x, y } => *mouse = vec2(x, y),
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.setup_pressed = zone_at(vec2(x, y), false);
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released = zone_at(vec2(x, y), false);
                    let armed = self.setup_pressed.take();
                    if let (Some(a), Some(r)) = (armed, released)
                        && a == r
                    {
                        if a.0 <= start_index {
                            self.setup_sel = a.0;
                            if cell_live(a.0, a.1) {
                                self.setup_cell = a.1;
                            }
                        }
                        activate = Some(a);
                        break;
                    }
                }
                RawEvent::TouchDown { id, x, y } if self.setup_pressed_touch.is_none() => {
                    *mouse = vec2(x, y);
                    self.setup_pressed_touch =
                        zone_at(*mouse, true).map(|(row, cell)| (id, row, cell));
                }
                RawEvent::TouchMove { id, x, y }
                    if self
                        .setup_pressed_touch
                        .is_some_and(|(finger, _, _)| finger == id) =>
                {
                    *mouse = vec2(x, y);
                }
                RawEvent::TouchUp { id, x, y }
                    if self
                        .setup_pressed_touch
                        .is_some_and(|(finger, _, _)| finger == id) =>
                {
                    *mouse = vec2(x, y);
                    let released = zone_at(*mouse, true);
                    let (_, row, cell) = self
                        .setup_pressed_touch
                        .take()
                        .expect("matching touch is armed");
                    let armed = (row, cell);
                    if released == Some(armed) {
                        if row <= start_index {
                            self.setup_sel = row;
                            if cell_live(row, cell) {
                                self.setup_cell = cell;
                            }
                        }
                        activate = Some(armed);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some((row, cell)) = activate {
            self.setup_pressed = None;
            self.setup_pressed_touch = None;
            if row == previous_page_index {
                self.setup_page = self.setup_page.saturating_sub(1);
                self.setup_sel = self.setup_page * COMPACT_PAGE_ITEMS;
                self.setup_cell = 0;
                sounds.push((SoundKind::Click, None));
                return None;
            }
            if row == next_page_index {
                self.setup_page = (self.setup_page + 1).min(layout.page_count - 1);
                self.setup_sel = (self.setup_page * COMPACT_PAGE_ITEMS).min(start_index);
                self.setup_cell = 0;
                sounds.push((SoundKind::Click, None));
                return None;
            }
            if row == start_index {
                // The sim refuses an all-one-team match (`OneTeam`:
                // nobody to fight, no way to win). Start reads
                // disabled and refuses here so the reason shows
                // inline instead of a failed-launch notice.
                if draft_one_team(draft) {
                    sounds.push((SoundKind::Denied, None));
                    return None;
                }
                sounds.push((SoundKind::Click, None));
                return Some(Out::Launch);
            }
            sounds.push((SoundKind::Click, None));
            let seat = order[row];
            match cell {
                // Seat choice never permutes seats or their other
                // choices; it moves the human's chair.
                0 => draft.seat_choice = seat,
                1 => {
                    let plan = &mut draft.seats[seat];
                    plan.difficulty = cycle_difficulty(plan.difficulty, 1);
                }
                2 => {
                    let plan = &mut draft.seats[seat];
                    plan.stance = cycle_stance(plan.stance, 1);
                }
                3 => {
                    let plan = &mut draft.seats[seat];
                    plan.faction_choice = (plan.faction_choice + 1) % FACTION_CHIP_ITEMS.len();
                }
                _ => {
                    // FFA, then every team up to the seat count
                    // (start_index is the full roster's length),
                    // wrapping back to FFA.
                    let plan = &mut draft.seats[seat];
                    plan.team_choice = (plan.team_choice + 1) % (start_index + 1);
                }
            }
        }
        None
    }

    /// Draws the setup screen: team-grouped seat cards, the Start
    /// button, and the live map with every chair marked.
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
        let tdims = measure_text(title, None, tsize as u16, 1.0);
        draw_text(
            title,
            (view.x - tdims.width) * 0.5,
            if compact { 58.0 } else { 64.0 * ui },
            tsize,
            TEXT_TITLE,
        );
        if !compact {
            let sub = format!(
                "{} - pick your seat, opponents, factions, and teams",
                scenario.name
            );
            let sdims = measure_text(&sub, None, (18.0 * ui) as u16, 1.0);
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
            let dims = measure_text(label, None, (17.0 * ui) as u16, 1.0);
            draw_rectangle(
                rect.x + dims.width + 12.0 * ui,
                rect.y + rect.h * 0.55,
                rect.w - dims.width - 12.0 * ui,
                1.0,
                Color::new(0.6, 0.6, 0.65, 0.25),
            );
        }
        for (pos, rect) in layout.seats.iter().enumerate() {
            if rect.w <= 0.0 {
                continue;
            }
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
            // Everything on a card scales to the card: a compressed
            // 400px-window roster once drew full-size discs and names
            // straight across its neighbors and chips.
            let disc = (10.0 * ui).min(rect.h * 0.38);
            if is_you {
                draw_circle_lines(chip_x, cy, disc * 1.3, 2.0, macroquad::prelude::WHITE);
            }
            draw_circle(chip_x, cy, disc, accent);
            let num = format!("{}", seat + 1);
            let num_font = (14.0 * ui).min(rect.h * 0.55);
            let ndims = measure_text(&num, None, num_font as u16, 1.0);
            draw_text(
                &num,
                chip_x - ndims.width * 0.5,
                cy + num_font * 0.35,
                num_font,
                Color::from_rgba(20, 20, 24, 255),
            );
            let mut name_font = (16.0 * ui).min(rect.h * 0.62);
            let text_right = if layout.bot_controls[pos][0].w > 0.0 {
                layout.bot_controls[pos][0].x
            } else {
                layout.cells[pos][0].x + layout.cells[pos][0].w
            };
            let name_room = (text_right - rect.x - 48.0 * ui).max(20.0);
            let nw = measure_text(&display, None, name_font as u16, 1.0).width;
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
                let tdims = measure_text(tag, None, tag_font as u16, 1.0);
                let fac = layout.cells[pos][1];
                draw_text(
                    tag,
                    fac.x - tdims.width - 14.0 * ui,
                    cy + tag_font * 0.35,
                    tag_font,
                    TEXT_SECONDARY,
                );
            }
            let bot_labels = [difficulty_name(plan.difficulty), stance_name(plan.stance)];
            for (index, (control, label)) in
                layout.bot_controls[pos].iter().zip(bot_labels).enumerate()
            {
                if control.w <= 0.0 {
                    continue;
                }
                let on_cell = selected && self.setup_cell == index + 1;
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
                let mut dims = measure_text(label, None, font as u16, 1.0);
                if dims.width > control.w - 6.0 {
                    font = (font * (control.w - 6.0) / dims.width).max(8.0);
                    dims = measure_text(label, None, font as u16, 1.0);
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
            let labels = [FACTION_CHIP_ITEMS[plan.faction_choice], team_label.as_str()];
            for (ci, label) in labels.iter().enumerate() {
                let chip = layout.cells[pos][ci + 1];
                if chip.w <= 0.0 {
                    continue;
                }
                let on_cell = selected && self.setup_cell == ci + 3;
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
                // The label fits ITS chip: squeezed cards shrink the
                // type instead of spilling text across neighbors.
                let mut font = 13.0 * ui;
                let mut ldims = measure_text(label, None, font as u16, 1.0);
                if ldims.width > chip.w - 6.0 {
                    font = (font * (chip.w - 6.0) / ldims.width).max(8.0);
                    ldims = measure_text(label, None, font as u16, 1.0);
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
            if selected && self.setup_cell == 0 && !is_you {
                let zone = layout.cells[pos][0];
                draw_rectangle(
                    zone.x + 44.0 * ui,
                    cy + name_font * 0.55,
                    measure_text(&display, None, name_font as u16, 1.0).width,
                    1.5,
                    TEXT_TITLE,
                );
            }
        }
        // Start button. An all-one-team draft cannot launch (the sim's
        // OneTeam refusal), so the button reads disabled.
        let one_team = draft_one_team(draft);
        let start_selected = self.setup_sel == layout.seats.len();
        if layout.start.w > 0.0 {
            draw_rectangle(
                layout.start.x,
                layout.start.y,
                layout.start.w,
                layout.start.h,
                SURFACE_MENU,
            );
            draw_rectangle_lines(
                layout.start.x,
                layout.start.y,
                layout.start.w,
                layout.start.h,
                if start_selected { 3.0 } else { 1.5 },
                if one_team {
                    Color::new(0.6, 0.6, 0.65, 0.4)
                } else if start_selected {
                    TEXT_TITLE
                } else {
                    TEXT_SECONDARY
                },
            );
            let label = "Start match";
            let ldims = measure_text(label, None, (20.0 * ui) as u16, 1.0);
            draw_text(
                label,
                layout.start.x + (layout.start.w - ldims.width) * 0.5,
                layout.start.y + layout.start.h * 0.66,
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
            let mut dims = measure_text(&label, None, size as u16, 1.0);
            if dims.width > rect.w - 8.0 {
                size = (size * (rect.w - 8.0) / dims.width).max(8.0);
                dims = measure_text(&label, None, size as u16, 1.0);
            }
            draw_text(
                &label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h * 0.5 + size * 0.35,
                size,
                TEXT_SECONDARY,
            );
        }

        if let Some((idx, entry)) = self.picked_entry(draft)
            && let Some(tex) = previews.get(idx, entry)
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
            let hint = if one_team {
                "every seat is on one team, nobody to fight - regroup a TEAM chip - Esc back"
            } else if self.setup_sel == order.len() {
                "Enter starts the match - Esc back"
            } else if self.setup_cell == 1 {
                "Enter cycles difficulty - Left/Right move - Esc back"
            } else if self.setup_cell == 2 {
                "Enter cycles stance - Left/Right move - Esc back"
            } else if self.setup_cell > 2 {
                "Enter cycles the chip - Left/Right move - Esc back"
            } else {
                "Enter takes this seat - Left/Right reach difficulty, stance, faction, and team - Esc back"
            };
            let hdims = measure_text(hint, None, (16.0 * ui) as u16, 1.0);
            draw_text(
                hint,
                (view.x - hdims.width) * 0.5,
                view.y - 20.0 * ui,
                16.0 * ui,
                if one_team {
                    TEXT_DANGER
                } else {
                    TEXT_SECONDARY
                },
            );
        }
    }

    /// The debug protocol's stable mode name for the current step —
    /// unchanged across redesigns, so automation scripts keep their
    /// footing.
    pub fn mode_name(&self) -> &'static str {
        match self.step {
            Step::Map => "main_menu",
            Step::Setup => "match_setup",
        }
    }

    /// The (title, items, selected) surface QueryUi reports — the
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
                                if seat == draft.seat_choice {
                                    format!(
                                        "{}. {} (you) | {} | {}",
                                        seat + 1,
                                        name,
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
                items.push("Start match".to_string());
                ("MATCH SETUP".to_string(), items, self.setup_sel)
            }
        }
    }

    /// The half-open item-index range actually on screen — QueryUi's
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
            Step::Setup => draft
                .scenario
                .as_deref()
                .map(|scenario| {
                    setup_layout_page(scenario, draft.seat_choice, view, ui, self.setup_page)
                        .visible_range
                })
                .unwrap_or([0, 0]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn press(key: Key) -> Vec<RawEvent> {
        vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
    }

    fn drive(w: &mut Wizard, draft: &mut NewMatchDraft, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        w.update(&press(key), &mut mouse, draft, &mut sounds)
            .expect("update")
    }

    /// Activates the browser's first entry (sections put duels first,
    /// so entry 0 is always a 1v1 — its own test pins that).
    fn pick_first_map(w: &mut Wizard, draft: &mut NewMatchDraft) {
        w.browser.selected = 0;
        assert_eq!(drive(w, draft, Key::Enter), Out::Stay);
    }

    fn explicit_team_setup() -> (Wizard, NewMatchDraft) {
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/trident-plateau.json"
        ));
        let scenario = Scenario::load(&path).expect("shipped team map");
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, Some(path));
        let mut wizard = Wizard::open(&draft);
        wizard.goto(Step::Setup, &draft);
        (wizard, draft)
    }

    #[test]
    fn every_discovered_map_lands_on_setup_and_launches_as_authored() {
        let count = Wizard::open(&NewMatchDraft::default()).entries.len();
        assert!(count > 0, "the browser always has an embedded fallback");

        for index in 0..count {
            let mut draft = NewMatchDraft::default();
            let mut wizard = Wizard::open(&draft);
            let expected_path = wizard.entries[index].path.clone();
            let expected_seats = wizard.entries[index].seats;
            wizard.browser.selected = index;

            assert_eq!(drive(&mut wizard, &mut draft, Key::Enter), Out::Stay);
            assert_eq!(wizard.step, Step::Setup, "map {index} skipped setup");
            assert_eq!(draft.scenario_path, expected_path, "map {index} changed");
            assert_eq!(draft.seats.len(), expected_seats, "map {index} seat count");
            assert!(
                draft.seats.iter().all(|plan| plan.faction_choice == 0),
                "map {index} did not preserve its authored factions"
            );
            assert_eq!(draft.seat_choice, 0, "map {index} did not open on seat 0");
            assert_eq!(
                wizard.setup_sel,
                draft.seats.len(),
                "map {index} did not preselect Start"
            );
            assert_eq!(
                drive(&mut wizard, &mut draft, Key::Enter),
                Out::Launch,
                "map {index} did not launch with its authored teams"
            );
        }
    }

    #[test]
    fn a_stale_seat_never_carries_across_maps() {
        // Take a late chair on a team map, back out, pick a duel: the
        // chair and choices must reset — the old clamp silently sat the
        // human in the duel's second seat with nothing on screen
        // saying so. Re-entering the SAME map keeps every answer.
        let team = Scenario::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/compass-grand.json"
        ))
        .expect("shipped map");
        let path = Some(PathBuf::from("compass-grand.json"));
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(team.clone(), path.clone());
        draft.seat_choice = 5;
        draft.seats[3].faction_choice = 2;
        draft.set_scenario(team, path);
        assert_eq!(draft.seat_choice, 5, "same map: the chair survives Back");
        assert_eq!(
            draft.seats[3].faction_choice, 2,
            "same map: choices survive"
        );
        draft.set_scenario(Scenario::skirmish(), None);
        assert_eq!(draft.seat_choice, 0, "new map: the chair resets");
        assert!(draft.seats.iter().all(|p| *p == SeatPlan::default()));
    }

    #[test]
    fn opponent_choices_start_from_the_map_and_follow_backtracking_rules() {
        let mut authored = Scenario::skirmish();
        authored.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Aggressive,
            0xDEAD_BEEF,
        ));
        let path = Some(PathBuf::from("authored-duel.json"));
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(authored.clone(), path.clone());
        assert_eq!(draft.seats[1].difficulty, BotDifficulty::Prime);
        assert_eq!(draft.seats[1].stance, BotStance::Aggressive);

        draft.seats[1].difficulty = BotDifficulty::Scrapheap;
        draft.seats[1].stance = BotStance::Turtle;
        draft.set_scenario(authored.clone(), path);
        assert_eq!(draft.seats[1].difficulty, BotDifficulty::Scrapheap);
        assert_eq!(draft.seats[1].stance, BotStance::Turtle);

        draft.set_scenario(authored, Some(PathBuf::from("another-copy.json")));
        assert_eq!(draft.seats[1].difficulty, BotDifficulty::Prime);
        assert_eq!(draft.seats[1].stance, BotStance::Aggressive);
    }

    #[test]
    fn the_team_chip_defaults_follow_the_authored_teams() {
        let team = Scenario::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/trident-plateau.json"
        ))
        .expect("shipped map");
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(team.clone(), None);
        let choices: Vec<usize> = draft.seats.iter().map(|p| p.team_choice).collect();
        assert_eq!(
            choices,
            vec![1, 1, 1, 2, 2, 2],
            "authored teams open as Team 1 / Team 2"
        );

        // Sparse authored ids still label densely by first appearance,
        // and an omitted seat opens as FFA — the same normalization the
        // sim applies at build.
        let mut sparse = team;
        sparse.players[0].team = Some(9);
        sparse.players[1].team = Some(9);
        sparse.players[2].team = None;
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(sparse, None);
        let choices: Vec<usize> = draft.seats.iter().map(|p| p.team_choice).collect();
        assert_eq!(choices, vec![1, 1, 0, 2, 2, 2]);

        let mut draft = NewMatchDraft::default();
        draft.set_scenario(Scenario::skirmish(), None);
        assert!(
            draft.seats.iter().all(|p| p.team_choice == 0),
            "a map without authored teams opens as FFA"
        );
    }

    #[test]
    fn the_team_chip_cycles_through_ffa_and_every_team() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        pick_first_map(&mut w, &mut draft); // a duel: FFA, Team 1, Team 2
        let order = seat_display_order(draft.scenario.as_deref().unwrap());

        // Your own card skips the absent opponent chip: Right lands on
        // faction, then team.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Right);
        assert_eq!(w.setup_cell, 4, "the team chip is the last cell");
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[draft.seat_choice].team_choice, 1,
            "FFA cycles to Team 1"
        );
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seats[draft.seat_choice].team_choice, 2);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[draft.seat_choice].team_choice, 0,
            "past the seat count wraps back to FFA"
        );
        assert_eq!(
            w.step,
            Step::Setup,
            "cycling a chip never leaves the screen"
        );

        // The sticky column carries the team cell onto an AI card.
        drive(&mut w, &mut draft, Key::Down);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seats[order[1]].team_choice, 1);
    }

    #[test]
    fn a_stale_team_choice_never_carries_across_maps() {
        // Same shape as the stale-seat guard: re-entering the SAME map
        // keeps the choice, a different map re-derives the authored
        // defaults — a Team 5 chosen on an 8-seat map must not ride
        // into a duel that has no Team 5.
        let team = Scenario::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/compass-grand.json"
        ))
        .expect("shipped map");
        let path = Some(PathBuf::from("compass-grand.json"));
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(team.clone(), path.clone());
        assert_eq!(
            draft.seats[7].team_choice, 2,
            "defaults follow the authored teams"
        );
        draft.seats[7].team_choice = 5;
        draft.set_scenario(team, path);
        assert_eq!(
            draft.seats[7].team_choice, 5,
            "same map: the team choice survives Back"
        );
        draft.set_scenario(Scenario::skirmish(), None);
        assert!(
            draft.seats.iter().all(|p| p.team_choice == 0),
            "new map: every team choice returns to that map's default"
        );
    }

    #[test]
    fn an_all_one_team_draft_disables_start() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        pick_first_map(&mut w, &mut draft);
        for plan in &mut draft.seats {
            plan.team_choice = 1;
        }
        drive(&mut w, &mut draft, Key::End);
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        let out = w
            .update(&press(Key::Enter), &mut mouse, &mut draft, &mut sounds)
            .expect("update");
        assert_eq!(out, Out::Stay, "one team, nobody to fight: Start refuses");
        assert_eq!(w.step, Step::Setup, "the screen stays put");
        assert!(
            sounds.contains(&(SoundKind::Denied, None)),
            "the refusal is audible, not silent"
        );
        draft.seats[0].team_choice = 0;
        assert_eq!(
            drive(&mut w, &mut draft, Key::Enter),
            Out::Launch,
            "freeing one seat re-arms Start"
        );
    }

    #[test]
    fn escape_unwinds_to_home_and_setup_steps_back_to_the_grid() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Home);

        let mut w = Wizard::open(&draft);
        pick_first_map(&mut w, &mut draft);
        assert_eq!(w.step, Step::Setup);
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Stay);
        assert_eq!(w.step, Step::Map, "Esc walks one step");
        assert_eq!(
            w.browser.selected,
            w.entries
                .iter()
                .position(|e| e.path == draft.scenario_path)
                .unwrap(),
            "the grid re-offers the earlier pick, found by PATH"
        );
    }

    #[test]
    fn the_browser_leads_with_duels() {
        let w = Wizard::open(&NewMatchDraft::default());
        assert!(
            w.entries.first().is_some_and(|e| e.seats == 2),
            "entry 0 must be a 1v1: a first Play+Enter never launches a team match"
        );
        let seat_counts: Vec<usize> = w.entries.iter().map(|e| e.seats).collect();
        let mut sorted = seat_counts.clone();
        sorted.sort_unstable();
        assert_eq!(seat_counts, sorted, "sections ascend by format");
    }

    #[test]
    fn a_team_map_runs_the_setup_screen_and_reseats_without_permuting() {
        let (mut w, mut draft) = explicit_team_setup();
        let seats = draft.seats.len();
        assert!(seats > 2, "the explicit fixture is a team map");
        assert_eq!(w.step, Step::Setup);
        assert_eq!(w.setup_sel, seats, "Start preselected under the seat cards");

        // Walk to the second DISPLAY seat; Enter takes the chair
        // inline — no sub-screen.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Down);
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seat_choice, order[1], "the chair moved");
        assert_eq!(w.step, Step::Setup, "and the screen never left");

        let (_, rows, _) = w.ui_surface(&draft);
        assert!(
            rows[0].contains("Difficulty Standard | Stance Balanced"),
            "every opponent plainly shows its configured difficulty and stance"
        );

        // End sits on Start; Enter launches.
        drive(&mut w, &mut draft, Key::End);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);

        draft.set_scenario(Scenario::skirmish(), None);
        assert_eq!(draft.seats.len(), 2, "re-derived at the new width");
        assert!(draft.seat_choice < 2, "the chair clamped onto the board");
    }

    #[test]
    fn omitted_singleton_teams_keep_their_setup_card() {
        let mut scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped");
        // Two explicit teammates then an omitted singleton — the shape
        // that used to derive two different surrogate keys and drop
        // the seat from the display order (Enter on Start then indexed
        // past the order and panicked).
        scenario.players[2].team = None;
        let order = seat_display_order(&scenario);
        assert_eq!(
            order.len(),
            scenario.players.len(),
            "every seat keeps a card"
        );
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, (0..scenario.players.len()).collect::<Vec<_>>());
        let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
        assert_eq!(layout.seats.len(), scenario.players.len());

        // An authored id inside the old surrogate range must not
        // swallow an omitted seat either.
        scenario.players[2].team = Some(202);
        let order = seat_display_order(&scenario);
        assert_eq!(order.len(), scenario.players.len());
    }

    #[test]
    fn the_faction_chip_cycles_on_every_card_including_yours() {
        let (mut w, mut draft) = explicit_team_setup();
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        assert_eq!(order[0], draft.seat_choice, "the human opens in seat 0");

        // Your own card: Right reaches the faction chip; Enter cycles
        // Auto to Ferrous.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[draft.seat_choice].faction_choice, 1,
            "your own chip cycled to Ferrous"
        );
        assert_eq!(
            w.step,
            Step::Setup,
            "cycling a chip never leaves the screen"
        );

        // The sticky column carries the faction cell onto an AI card.
        drive(&mut w, &mut draft, Key::Down);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seats[order[1]].faction_choice, 1);
        // Left from faction walks the two visible bot controls before the
        // seat action. The hidden seeded identity is never exposed.
        drive(&mut w, &mut draft, Key::Left);
        assert_eq!(w.setup_cell, 2);
        drive(&mut w, &mut draft, Key::Left);
        assert_eq!(w.setup_cell, 1);
        drive(&mut w, &mut draft, Key::Left);
        assert_eq!(w.setup_cell, 0);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seat_choice, order[1],
            "the next cell left of faction is the seat"
        );
    }

    #[test]
    fn keyboard_cycles_difficulty_and_stance_directly_and_independently() {
        let (mut wizard, mut draft) = explicit_team_setup();
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        let opponent = order[1];
        let original = draft.seats[opponent];
        drive(&mut wizard, &mut draft, Key::Home);
        drive(&mut wizard, &mut draft, Key::Down);
        drive(&mut wizard, &mut draft, Key::Right);
        assert_eq!(wizard.setup_cell, 1);
        drive(&mut wizard, &mut draft, Key::Enter);

        assert_eq!(wizard.mode_name(), "match_setup");
        assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[opponent].stance, original.stance);
        assert_eq!(
            draft.seats[opponent].faction_choice,
            original.faction_choice
        );
        assert_eq!(draft.seats[opponent].team_choice, original.team_choice);

        drive(&mut wizard, &mut draft, Key::Right);
        assert_eq!(wizard.setup_cell, 2);
        drive(&mut wizard, &mut draft, Key::Enter);
        assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[opponent].stance, BotStance::Aggressive);
        assert_eq!(
            draft.seats[opponent].faction_choice,
            original.faction_choice
        );
        assert_eq!(draft.seats[opponent].team_choice, original.team_choice);

        let (title, items, selected) = wizard.ui_surface(&draft);
        assert_eq!(title, "MATCH SETUP");
        assert!(items[1].contains("Difficulty Veteran"));
        assert!(items[1].contains("Stance Aggressive"));
        assert_eq!(selected, 1);
        assert!(items.iter().all(|item| !item.contains("seed")));
    }

    #[test]
    fn direct_bot_control_activation_ends_the_input_batch() {
        let (mut wizard, mut draft) = explicit_team_setup();
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        let opponent = order[1];
        wizard.setup_sel = 1;
        wizard.setup_cell = 1;
        wizard.setup_page = 0;

        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();
        let out = wizard
            .update(
                &[
                    RawEvent::KeyDown { key: Key::Enter },
                    RawEvent::KeyDown { key: Key::Right },
                    RawEvent::KeyDown { key: Key::Enter },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("activate direct difficulty control");
        assert_eq!(out, Out::Stay);
        assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[opponent].stance, BotStance::Balanced);
        assert_eq!(wizard.setup_cell, 1);
        assert_eq!(wizard.mode_name(), "match_setup");
        assert_eq!(sounds, [(SoundKind::Click, None)]);
    }

    #[test]
    fn mouse_and_touch_cycle_the_direct_bot_controls() {
        crate::render::set_viewport(640.0, 400.0);
        let (mut wizard, mut draft) = explicit_team_setup();
        let scenario = draft.scenario.as_deref().expect("picked scenario");
        let order = seat_display_order(scenario);
        let row = 1;
        let opponent = order[row];
        wizard.setup_sel = row;
        wizard.setup_page = 0;
        let setup = setup_layout(
            scenario,
            draft.seat_choice,
            crate::render::viewport(),
            crate::render::ui_scale(),
        );
        let difficulty = setup.bot_controls[row][0].center();
        let stance = setup.bot_controls[row][1].center();
        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();
        wizard
            .update(
                &[
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x: difficulty.x,
                        y: difficulty.y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x: difficulty.x,
                        y: difficulty.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("cycle difficulty");
        assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[opponent].stance, BotStance::Balanced);
        assert_eq!(wizard.mode_name(), "match_setup");
        assert_eq!(wizard.setup_cell, 1);

        wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 4,
                        x: stance.x,
                        y: stance.y,
                    },
                    RawEvent::TouchUp {
                        id: 4,
                        x: stance.x,
                        y: stance.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("cycle stance");
        assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[opponent].stance, BotStance::Aggressive);
        assert_eq!(wizard.mode_name(), "match_setup");
        assert_eq!(wizard.setup_cell, 2);
        assert_eq!(sounds, [(SoundKind::Click, None); 2]);
    }

    #[test]
    fn compact_setup_pages_to_a_nonoverlapping_opponent_touch_target() {
        crate::render::set_viewport(640.0, 400.0);
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/compass-grand.json"
        ));
        let scenario = Scenario::load(&path).expect("shipped eight-seat map");
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, Some(path));
        let mut wizard = Wizard::open(&draft);
        wizard.goto(Step::Setup, &draft);
        assert_eq!(
            wizard.setup_page, 1,
            "Start opens on the page that contains it"
        );

        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let last_page = setup_layout_page(
            draft.scenario.as_deref().unwrap(),
            draft.seat_choice,
            view,
            ui,
            wizard.setup_page,
        );
        let previous = last_page
            .page_prev
            .expect("another opponent page precedes Start");
        assert!(previous.h >= MIN_TOUCH_TARGET);
        let at = previous.center();
        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();
        wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 20,
                        x: at.x,
                        y: at.y,
                    },
                    RawEvent::TouchUp {
                        id: 20,
                        x: at.x,
                        y: at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .unwrap();
        assert_eq!(wizard.setup_page, 0);
        assert_eq!(wizard.setup_sel, 0);

        let first_page = setup_layout_page(
            draft.scenario.as_deref().unwrap(),
            draft.seat_choice,
            view,
            ui,
            wizard.setup_page,
        );
        let row = 1;
        let difficulty = first_page.bot_controls[row][0];
        let touch = setup_touch_cell_rect(&first_page, row, 1).unwrap();
        assert!(touch.h >= MIN_TOUCH_TARGET);
        let at = vec2(touch.center().x, touch.y + 1.0);
        assert!(
            !difficulty.contains(at),
            "the probe exercises the enlarged touch-only area"
        );
        wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 21,
                        x: at.x,
                        y: at.y,
                    },
                    RawEvent::TouchUp {
                        id: 21,
                        x: at.x,
                        y: at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .unwrap();
        let seat = seat_display_order(draft.scenario.as_deref().unwrap())[row];
        assert_eq!(wizard.mode_name(), "match_setup");
        assert_eq!(draft.seats[seat].difficulty, BotDifficulty::Veteran);
        assert_eq!(draft.seats[seat].stance, BotStance::Balanced);
    }

    #[test]
    fn compact_setup_touch_only_edges_activate_every_editable_chip() {
        crate::render::set_viewport(640.0, 400.0);
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/compass-grand.json"
        ));

        for cell in 1..=4 {
            let scenario = Scenario::load(&path).expect("shipped eight-seat map");
            let mut draft = NewMatchDraft::default();
            draft.set_scenario(scenario, Some(path.clone()));
            let order = seat_display_order(draft.scenario.as_deref().unwrap());
            let row = 1;
            let seat = order[row];
            assert_ne!(seat, draft.seat_choice);
            let initial_difficulty = draft.seats[seat].difficulty;
            let initial_stance = draft.seats[seat].stance;
            let initial_faction = draft.seats[seat].faction_choice;
            let initial_team = draft.seats[seat].team_choice;
            let mut wizard = Wizard::open(&draft);
            wizard.goto(Step::Setup, &draft);
            wizard.setup_sel = row;
            wizard.setup_page = 0;

            let layout = setup_layout_page(
                draft.scenario.as_deref().unwrap(),
                draft.seat_choice,
                crate::render::viewport(),
                crate::render::ui_scale(),
                0,
            );
            let visual = setup_cell_rect(&layout, row, cell).unwrap();
            let touch = setup_touch_cell_rect(&layout, row, cell).unwrap();
            let at = vec2(touch.center().x, touch.y + 1.0);
            assert!(
                !visual.contains(at),
                "cell {cell} probe must exercise its touch-only edge"
            );

            let mut mouse = Vec2::ZERO;
            let mut sounds = Vec::new();
            let out = wizard
                .update(
                    &[
                        RawEvent::TouchDown {
                            id: cell as u64,
                            x: at.x,
                            y: at.y,
                        },
                        RawEvent::TouchUp {
                            id: cell as u64,
                            x: at.x,
                            y: at.y,
                        },
                    ],
                    &mut mouse,
                    &mut draft,
                    &mut sounds,
                )
                .expect("activate compact setup cell");
            assert_eq!(out, Out::Stay);
            assert_eq!(sounds, [(SoundKind::Click, None)]);
            match cell {
                1 => {
                    assert_eq!(
                        draft.seats[seat].difficulty,
                        cycle_difficulty(initial_difficulty, 1)
                    );
                    assert_eq!(draft.seats[seat].stance, initial_stance);
                }
                2 => assert_eq!(draft.seats[seat].stance, cycle_stance(initial_stance, 1)),
                3 => assert_eq!(
                    draft.seats[seat].faction_choice,
                    (initial_faction + 1) % FACTION_CHIP_ITEMS.len()
                ),
                4 => assert_eq!(
                    draft.seats[seat].team_choice,
                    (initial_team + 1) % (draft.seats.len() + 1)
                ),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn setup_mouse_activation_requires_release_on_the_armed_cell() {
        crate::render::set_viewport(1280.0, 800.0);
        let (mut wizard, mut draft) = explicit_team_setup();
        let scenario = draft.scenario.as_deref().expect("picked scenario");
        let order = seat_display_order(scenario);
        let layout = setup_layout(
            scenario,
            draft.seat_choice,
            crate::render::viewport(),
            crate::render::ui_scale(),
        );
        let row = 1;
        let seat = order[row];
        let seat_at = layout.cells[row][0].center();
        let faction_at = layout.cells[row][1].center();
        let team_at = layout.cells[row][2].center();
        let start_at = layout.start.center();
        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();

        let out = wizard
            .update(
                &[
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x: faction_at.x,
                        y: faction_at.y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x: team_at.x,
                        y: team_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(out, Out::Stay);
        assert_eq!(draft.seats[seat].faction_choice, 0);
        assert!(sounds.is_empty(), "a canceled click is silent");

        let out = wizard
            .update(
                &[
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x: faction_at.x,
                        y: faction_at.y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x: faction_at.x,
                        y: faction_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(out, Out::Stay);
        assert_eq!(draft.seats[seat].faction_choice, 1);
        assert_eq!(wizard.setup_sel, row);
        assert_eq!(wizard.setup_cell, 3);

        let out = wizard
            .update(
                &[
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x: seat_at.x,
                        y: seat_at.y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x: seat_at.x,
                        y: seat_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(out, Out::Stay);
        assert_eq!(draft.seat_choice, seat, "the clicked chair becomes human");

        let out = wizard
            .update(
                &[
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x: start_at.x,
                        y: start_at.y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x: start_at.x,
                        y: start_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(out, Out::Launch);
        assert_eq!(sounds.len(), 3, "each committed click has one cue");
    }

    #[test]
    fn setup_touch_activation_belongs_to_its_first_finger_and_armed_cell() {
        crate::render::set_viewport(1280.0, 800.0);
        let (mut wizard, mut draft) = explicit_team_setup();
        let scenario = draft.scenario.as_deref().expect("picked scenario");
        let order = seat_display_order(scenario);
        let layout = setup_layout(
            scenario,
            draft.seat_choice,
            crate::render::viewport(),
            crate::render::ui_scale(),
        );
        let row = 1;
        let seat = order[row];
        let faction = layout.cells[row][1];
        let faction_at = faction.center();
        let team_at = layout.cells[row][2].center();
        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();

        wizard
            .update(
                &[RawEvent::TouchDown {
                    id: 7,
                    x: faction_at.x,
                    y: faction_at.y,
                }],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(wizard.setup_pressed_touch, Some((7, row, 3)));
        assert_eq!(mouse, faction_at);

        // A second finger cannot steal or resolve the first finger's press.
        wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 8,
                        x: team_at.x,
                        y: team_at.y,
                    },
                    RawEvent::TouchUp {
                        id: 8,
                        x: team_at.x,
                        y: team_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(wizard.setup_pressed_touch, Some((7, row, 3)));
        assert_eq!(mouse, faction_at);

        // The owner releases over another cell, so the gesture cancels.
        wizard
            .update(
                &[
                    RawEvent::TouchMove {
                        id: 7,
                        x: team_at.x,
                        y: team_at.y,
                    },
                    RawEvent::TouchUp {
                        id: 7,
                        x: team_at.x,
                        y: team_at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(wizard.setup_pressed_touch, None);
        assert_eq!(draft.seats[seat].faction_choice, 0);
        assert!(sounds.is_empty());

        // A fresh gesture may move within the armed cell and still commit.
        let inside = vec2(faction.x + 2.0, faction.y + 2.0);
        let out = wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 9,
                        x: faction_at.x,
                        y: faction_at.y,
                    },
                    RawEvent::TouchMove {
                        id: 9,
                        x: inside.x,
                        y: inside.y,
                    },
                    RawEvent::TouchUp {
                        id: 9,
                        x: inside.x,
                        y: inside.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("update");
        assert_eq!(out, Out::Stay);
        assert_eq!(draft.seats[seat].faction_choice, 1);
        assert_eq!(wizard.setup_sel, row);
        assert_eq!(wizard.setup_cell, 3);
        assert_eq!(sounds, vec![(SoundKind::Click, None)]);
    }

    #[test]
    fn the_setup_layout_fits_the_smallest_supported_window() {
        // Eight seats at the supported extremes use two compact pages. Every
        // visible item and page control keeps a 44px logical target instead
        // of compressing the full roster into overlapping slivers.
        let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
        for (view, ui) in [
            (vec2(640.0, 400.0), 0.75),
            (vec2(640.0, 400.0), 1.0),
            (vec2(960.0, 400.0), 1.5),
        ] {
            let pages = [
                setup_layout_page(&scenario, 0, view, ui, 0),
                setup_layout_page(&scenario, 0, view, ui, 1),
            ];
            assert_eq!(pages[0].page_count, 2);
            assert_eq!(pages[0].visible_range, [0, 5]);
            assert_eq!(pages[1].visible_range, [5, 9]);
            assert!(pages[0].page_next.is_some_and(|rect| rect.h >= 44.0));
            assert!(pages[1].page_prev.is_some_and(|rect| rect.h >= 44.0));
            assert!(pages[1].start.h >= 44.0);
            assert!(pages[1].start.y + pages[1].start.h <= view.y);

            let mut seen = Vec::new();
            for layout in &pages {
                for (pos, card) in layout
                    .seats
                    .iter()
                    .enumerate()
                    .filter(|(_, card)| card.w > 0.0)
                {
                    seen.push(pos);
                    assert!(card.h >= 44.0, "visible cards keep a 44px target");
                    assert!(card.y + card.h <= view.y);
                    assert!(layout.cells[pos][0].w > 24.0);
                    let touches: Vec<Rect> = (0..5)
                        .filter_map(|cell| setup_touch_cell_rect(layout, pos, cell))
                        .collect();
                    for touch in &touches {
                        assert!(
                            touch.w >= MIN_TOUCH_TARGET && touch.h >= MIN_TOUCH_TARGET,
                            "every semantic setup target stays at least 44px: {touch:?}"
                        );
                        assert_eq!((touch.y, touch.h), (card.y, card.h));
                        assert!(touch.x >= card.x);
                        assert!(touch.x + touch.w <= card.x + card.w + 0.01);
                    }
                    for pair in touches.windows(2) {
                        assert!(
                            pair[0].x + pair[0].w <= pair[1].x + 0.01,
                            "semantic setup targets never overlap"
                        );
                    }
                    for chip in layout.cells[pos].iter().skip(1).filter(|chip| chip.w > 0.0) {
                        assert!(chip.h <= card.h + 0.01);
                        assert!(chip.x >= card.x);
                    }
                }
            }
            seen.sort_unstable();
            assert_eq!(seen, (0..8).collect::<Vec<_>>());
        }
    }

    #[test]
    fn the_setup_layout_never_overlaps_its_own_parts() {
        let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
        let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
        assert_eq!(layout.seats.len(), 8);
        for pair in layout.seats.windows(2) {
            assert!(
                pair[0].y + pair[0].h <= pair[1].y + 0.01,
                "seat cards stack without overlap"
            );
        }
        let last = layout.seats.last().unwrap();
        assert!(
            layout.start.y >= last.y + last.h,
            "Start sits under the cards"
        );
        assert!(
            layout.preview.x >= last.x + last.w,
            "the preview never crosses the cards"
        );
        assert!(
            layout.start.y + layout.start.h <= 800.0,
            "everything fits an 800px window"
        );
        for (pos, cells) in layout.cells.iter().enumerate() {
            let card = layout.seats[pos];
            for r in cells.iter().filter(|r| r.w > 0.0) {
                assert!(
                    r.x >= card.x - 0.01
                        && r.y >= card.y - 0.01
                        && r.x + r.w <= card.x + card.w + 0.01
                        && r.y + r.h <= card.y + card.h + 0.01,
                    "cell rects nest inside their card"
                );
            }
            assert!(
                cells[1].w > 0.0 && cells[2].w > 0.0,
                "every card carries faction and team chips"
            );
            let controls = layout.bot_controls[pos];
            if seat_display_order(&scenario)[pos] == 0 {
                assert_eq!(controls, [Rect::new(0.0, 0.0, 0.0, 0.0); 2]);
            } else {
                assert!(
                    controls.iter().all(|control| control.w > 0.0),
                    "every opponent carries separate difficulty and stance controls"
                );
                assert!(
                    cells[0].x + cells[0].w <= controls[0].x + 0.01,
                    "the bot controls never overlap the take-seat control"
                );
                for control in controls {
                    assert!(
                        control.x >= card.x
                            && control.x + control.w <= card.x + card.w
                            && control.y >= card.y
                            && control.y + control.h <= card.y + card.h,
                        "each bot control nests inside its card"
                    );
                }
                assert!(
                    controls[0].x + controls[0].w <= controls[1].x,
                    "difficulty and stance controls never overlap"
                );
            }
        }
    }

    #[test]
    fn seat_anchors_reads_the_authored_digits() {
        let map: Vec<String> = vec!["####".into(), "#1.#".into(), "#.2#".into()];
        assert_eq!(seat_anchors(&map), vec![(0, (1, 1)), (1, (2, 2))]);
    }

    #[test]
    fn the_setup_card_and_its_protocol_row_show_the_retinted_name() {
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(Scenario::skirmish(), None);
        let sc = draft.scenario.as_deref().unwrap().clone();

        // Auto keeps the authored names, both seats.
        assert_eq!(effective_name(&sc, &draft, 0), "Ferrous");
        assert_eq!(effective_name(&sc, &draft, 1), "Cupric");

        // Overrides retint the label with the disc, both directions.
        draft.seats[0].faction_choice = 2; // Cupric
        draft.seats[1].faction_choice = 1; // Ferrous
        assert_eq!(effective_name(&sc, &draft, 0), "Cupric");
        assert_eq!(effective_name(&sc, &draft, 1), "Ferrous");

        // QueryUi speaks the same name: the card and the automation
        // surface can't disagree.
        let mut w = Wizard::open(&draft);
        w.step = Step::Setup;
        let (_, items, _) = w.ui_surface(&draft);
        assert_eq!(items[0], "1. Cupric (you) | Cupric | FFA");
        assert_eq!(
            items[1], "2. Ferrous | Difficulty Standard | Stance Balanced | Ferrous | FFA",
            "the protocol row matches the visible opponent card"
        );
    }

    #[test]
    fn the_previewed_name_is_the_launched_name() {
        // The regression guard: the preview reads the SAME rule launch
        // applies (Scenario::retint_seat). Reimplementing the rename in
        // the shell — where a name without a faction word diverges —
        // fails here.
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(Scenario::skirmish(), None);
        draft.seats[0].faction_choice = 2; // Cupric
        draft.seats[1].faction_choice = 1; // Ferrous
        let sc = draft.scenario.as_deref().unwrap().clone();
        for seat in 0..sc.players.len() {
            let previewed = effective_name(&sc, &draft, seat);
            let mut launched = sc.clone();
            launched.retint_seat(seat, effective_faction(&sc, &draft, seat));
            assert_eq!(
                previewed, launched.players[seat].name,
                "seat {seat}: the card promised a name launch didn't deliver"
            );
        }
    }

    fn grid_entries(n: usize) -> Vec<ScenarioEntry> {
        (0..n)
            .map(|i| ScenarioEntry {
                seats: 2,
                label: format!("m{i}"),
                blurb: None,
                path: Some(PathBuf::from(format!("m{i}.json"))),
                theme: String::new(),
            })
            .collect()
    }

    #[test]
    fn the_map_grids_visible_range_is_the_real_window() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        w.entries = grid_entries(24);
        w.browser = Browser::new();

        // A small window clips the grid; the range must say so and
        // must contain the selection End just scrolled to.
        crate::render::set_viewport(640.0, 400.0);
        drive(&mut w, &mut draft, Key::End);
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let [first, past] = w.ui_visible_range(&draft, view, ui);
        assert!(past <= w.entries.len());
        assert!(
            past - first < w.entries.len(),
            "a 640x400 window cannot show all 24 cards ([{first}, {past}])"
        );
        assert!(
            (first..past).contains(&w.browser.selected),
            "the selection sits inside the reported window"
        );

        // A huge window shows the whole shelf; the resize guard runs
        // on the next handled frame, like the live loop.
        crate::render::set_viewport(2000.0, 4000.0);
        let mut mouse = vec2(0.0, 0.0);
        let _ = w.update(
            &[RawEvent::MouseMove { x: 0.0, y: 0.0 }],
            &mut mouse,
            &mut draft,
            &mut Vec::new(),
        );
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        assert_eq!(
            w.ui_visible_range(&draft, view, ui),
            [0, w.entries.len()],
            "a window tall enough for everything reports everything"
        );
    }

    #[test]
    fn an_empty_grid_reports_an_empty_window() {
        let draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        w.entries.clear();
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        assert_eq!(w.ui_visible_range(&draft, view, ui), [0, 0]);
    }

    #[test]
    fn compact_setup_reports_only_the_page_actually_on_screen() {
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scenarios/compass-grand.json"
        ));
        let scenario = Scenario::load(&path).expect("shipped eight-seat map");
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, Some(path));
        let mut w = Wizard::open(&draft);
        w.goto(Step::Setup, &draft);
        crate::render::set_viewport(640.0, 400.0);
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let len = w.ui_surface(&draft).1.len();
        assert_eq!(len, 9);
        assert_eq!(w.ui_visible_range(&draft, view, ui), [5, 9]);
        drive(&mut w, &mut draft, Key::Home);
        assert_eq!(w.ui_visible_range(&draft, view, ui), [0, 5]);
    }
}
