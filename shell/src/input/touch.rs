//! Gameplay touch gestures: tap, long-press, one-finger pan, the
//! two-finger pinch and box, and the ring that charges under a resting
//! finger. Every finger arrives through the same semantic event funnel
//! as the mouse, in logical pixels.

use super::*;

/// Where a finger landed, which decides what it may drive for its
/// whole life. Chrome and minimap fingers never drive world gestures:
/// a swipe starting on the command panel must not pan the camera
/// behind it, and a two-finger box with a chrome corner must not select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchBorn {
    /// Open battlefield.
    World,
    /// The minimap.
    Minimap,
    /// Any other HUD chrome.
    Chrome,
}

/// One live finger on the screen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TouchPoint {
    /// Where it landed.
    pub origin: Vec2,
    /// Where it is now.
    pub at: Vec2,
    /// Wall clock at touch-down (the injected `now`).
    pub down_at: f64,
    /// Where it landed.
    pub born: TouchBorn,
    /// Whether it ever left the slop circle — a moved finger is a
    /// drag, never a tap or a long-press.
    pub moved: bool,
    /// Whether it already did its one job: its long-press fired, or it
    /// outlived its pair. A spent finger never taps or long-presses.
    pub spent: bool,
    /// Whether it ever belonged to a two-finger pair.
    pub paired: bool,
    /// The card it landed on, as it stood then.
    pub card: Option<PressedCard>,
}

/// A card as a finger found it: its slot, its action, and its face. A
/// resting finger lets the match change the panel under it (a queue
/// shifts, a disabled card enables), so a lift activates only if it
/// finds this same card, not merely the same slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PressedCard {
    hit: crate::layout::CardHit,
    icon: Option<crate::panel::CardIcon>,
}

/// The card under a fingertip at `p`, if any.
fn pressed_card(game: &Game, p: Vec2, ui: f32) -> Option<PressedCard> {
    let hit = crate::layout::card_under(&game.presentation.layout.get(), p, Some(ui))?;
    let icon = game
        .presentation
        .panel_model
        .borrow()
        .as_ref()
        .and_then(|panel| panel.card(hit.row, hit.index))
        .map(|card| card.icon);
    Some(PressedCard { hit, icon })
}

/// A pair finger the platform reported lifted. iOS reports every live
/// finger lifted when any one of them lifts, so a finger remembered
/// here may still be on the glass; its next move picks it back up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiftedFinger {
    id: u64,
    born: TouchBorn,
    moved: bool,
}

/// How many lifted pair fingers are remembered: a pair has two.
const LIFTED_MEMORY: usize = 2;

impl TouchPoint {
    /// A still, unspent finger: its lift may still be a tap, and on the
    /// battlefield its rest may still charge a long-press.
    fn still(&self) -> bool {
        !self.moved && !self.spent
    }
}

/// What a two-finger pair is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairState {
    /// Neither a pinch nor anything else yet; a lift commits a box.
    Undecided,
    /// The spread changed past the threshold: zooming for the pair's
    /// whole lifetime, so lifting one finger commits no box.
    Pinch,
    /// A finger landed off the battlefield: the pair neither zooms
    /// nor boxes.
    Inert,
}

/// A live two-finger gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pair {
    /// The fingers' spread when the pair formed. Pinch detection
    /// compares the CUMULATIVE change against it, so a slow pinch
    /// (under a pixel per event) still reads as one.
    pub start_dist: f32,
    pub state: PairState,
}

/// The lone finger that may charge a battlefield long-press.
fn world_hold(input: &InputState) -> Option<TouchPoint> {
    let [(_, finger)] = input.touches.as_slice() else {
        return None;
    };
    (finger.born == TouchBorn::World && finger.still()).then_some(*finger)
}

/// Whether an armed mode takes a minimap tap as its target (a rally
/// point or a patrol waypoint), so the minimap must not steer under it.
fn minimap_targets(input: &InputState) -> bool {
    !input.rallying.is_empty() || input.patrol_route.is_some()
}

/// Points the camera at the minimap spot under a steering finger.
fn steer_minimap(game: &mut Game, p: Vec2) {
    if let Some(world) = crate::render::minimap_world_clamped(&game.view(), p) {
        game.presentation.camera.center = world;
        game.presentation.camera.pan(Vec2::ZERO); // re-clamp
    }
}

/// Where a finger landing at `p` was born.
fn born_at(game: &Game, p: Vec2) -> TouchBorn {
    if crate::render::minimap_world_at(&game.view(), p).is_some() {
        TouchBorn::Minimap
    } else if click_on_hud(game, p) {
        TouchBorn::Chrome
    } else {
        TouchBorn::World
    }
}

/// How long a finger must rest before it reads as deliberate rather
/// than the start of a tap, so feedback never flashes under quick taps.
pub(crate) const TOUCH_REST_MS: f64 = 120.0;

/// Where a battlefield long-press is charging and how full it is, from
/// zero once the finger has rested to one as the order fires. Only a
/// lone world-born finger that has neither moved nor fired charges.
pub(crate) fn long_press_progress(input: &InputState) -> Option<(Vec2, f32)> {
    let finger = world_hold(input)?;
    let held_ms = (input.now - finger.down_at) * 1000.0;
    if held_ms < TOUCH_REST_MS {
        return None;
    }
    let charge_ms = (f64::from(input.touch_prefs.long_press_ms) - TOUCH_REST_MS).max(1.0);
    let progress = ((held_ms - TOUCH_REST_MS) / charge_ms).min(1.0);
    Some((finger.at, progress as f32))
}

/// A finger landed.
pub(super) fn down(game: &mut Game, input: &mut InputState, id: u64, p: Vec2) {
    // iOS re-reports every live finger as landing when another lands.
    // A repeat is the same finger, not a fresh one: it keeps where it
    // was born, what it has done, and the pair it belongs to.
    if let Some((_, finger)) = input.touches.iter_mut().find(|(tid, _)| *tid == id) {
        finger.at = p;
        return;
    }
    // A genuine landing reuses no memory, even if the platform reused
    // the id of a finger lifted earlier.
    input.lifted_pair.retain(|lifted| lifted.id != id);
    let born = born_at(game, p);
    input.touches.push((
        id,
        TouchPoint {
            origin: p,
            at: p,
            down_at: input.now,
            born,
            moved: false,
            spent: false,
            card: pressed_card(game, p, input.ui),
            paired: false,
        },
    ));
    if input.touches.len() > 2 {
        // Three fingers mean nothing yet; the oldest yields.
        input.touches.remove(0);
    }
    // A finger landing on the minimap jumps the camera there at once,
    // like a mouse press, and steers it for as long as it drags.
    if born == TouchBorn::Minimap && !minimap_targets(input) {
        steer_minimap(game, p);
    }
    // A fresh pair starts undecided, whatever the last pair was doing:
    // a pinch must not outlive its fingers and swallow the next box.
    input.pair = (input.touches.len() == 2).then(|| {
        let [(_, a), (_, b)] = [input.touches[0], input.touches[1]];
        Pair {
            start_dist: (a.at - b.at).length(),
            state: if a.born == TouchBorn::World && b.born == TouchBorn::World {
                PairState::Undecided
            } else {
                PairState::Inert
            },
        }
    });
    if input.pair.is_some() {
        for (_, finger) in &mut input.touches {
            finger.paired = true;
        }
    }
}

/// Picks back up a pair finger the platform reported lifted while it
/// stayed down. It returns spent, keeping where it was born, so it can
/// pan or steer again but never tap or long-press.
fn readopt(input: &mut InputState, id: u64, p: Vec2) -> bool {
    if input.touches.len() >= 2 {
        return false;
    }
    let Some(index) = input.lifted_pair.iter().position(|lifted| lifted.id == id) else {
        return false;
    };
    let lifted = input.lifted_pair.remove(index);
    input.touches.push((
        id,
        TouchPoint {
            origin: p,
            at: p,
            down_at: input.now,
            born: lifted.born,
            moved: lifted.moved,
            spent: true,
            paired: false,
            card: None,
        },
    ));
    true
}

/// A finger moved.
pub(super) fn moved(game: &mut Game, input: &mut InputState, id: u64, p: Vec2) {
    // A move from a finger nobody is tracking belongs to a gesture this
    // screen never owned (the tutorial card swallowed its landing), or
    // to a pair finger iOS falsely reported lifted.
    if !input.touches.iter().any(|(tid, _)| *tid == id) && !readopt(input, id, p) {
        return;
    }
    let slop = click_slop(input.ui) * 2.0;
    let two = input.touches.len() == 2;
    let old_dist = two.then(|| (input.touches[0].1.at - input.touches[1].1.at).length());
    let mut delta = Vec2::ZERO;
    let mut born = TouchBorn::World;
    if let Some((_, tp)) = input.touches.iter_mut().find(|(tid, _)| *tid == id) {
        delta = p - tp.at;
        tp.at = p;
        born = tp.born;
        if (p - tp.origin).length() > slop {
            tp.moved = true;
        }
    }
    if born == TouchBorn::Minimap {
        if !minimap_targets(input) {
            steer_minimap(game, p);
        }
        return;
    }
    match input.touches.len() {
        // One moved finger drags the world under the hand —
        // unless it landed on chrome, whose ground it keeps.
        1 if input.touches[0].1.moved && input.touches[0].1.born == TouchBorn::World => {
            game.presentation.camera.center -= delta / game.presentation.camera.zoom;
            game.presentation.camera.pan(Vec2::ZERO); // re-clamp
        }
        // Two fingers: a spread that has CUMULATIVELY moved
        // past the threshold is a pinch (zoom at the
        // midpoint) — per-event deltas would miss a slow
        // pinch entirely and mis-commit it as a box select.
        2 => {
            let new_dist = (input.touches[0].1.at - input.touches[1].1.at).length();
            if let Some(pair) = &mut input.pair
                && pair.state == PairState::Undecided
                && (new_dist - pair.start_dist).abs()
                    > crate::viewer_touch::PINCH_START_PX * input.ui
            {
                pair.state = PairState::Pinch;
            }
            if input
                .pair
                .is_some_and(|pair| pair.state == PairState::Pinch)
                && let Some(old) = old_dist
            {
                let spread = new_dist - old;
                if spread != 0.0 {
                    let mid = (input.touches[0].1.at + input.touches[1].1.at) * 0.5;
                    game.presentation
                        .camera
                        .zoom_at(mid, spread * crate::viewer_touch::PINCH_NOTCHES_PER_PX);
                }
            }
        }
        _ => {}
    }
}

/// A finger lifted.
pub(super) fn up(game: &mut Game, input: &mut InputState, id: u64, p: Vec2) {
    let Some(pos) = input.touches.iter().position(|(tid, _)| *tid == id) else {
        // The real lift of a finger already reported lifted.
        input.lifted_pair.retain(|lifted| lifted.id != id);
        return;
    };
    let (_, lifted) = input.touches.remove(pos);
    if lifted.paired {
        if input.lifted_pair.len() == LIFTED_MEMORY {
            input.lifted_pair.remove(0);
        }
        input.lifted_pair.push(LiftedFinger {
            id,
            born: lifted.born,
            moved: lifted.moved,
        });
    }
    match input.touches.len() {
        // Second finger of a pair released: a pair that
        // never pinched commits the box between the fingers
        // — both corners world-born; a chrome-born finger
        // boxes nothing behind its panel.
        1 => {
            let survivor = input.touches[0].1;
            if input
                .pair
                .is_some_and(|pair| pair.state == PairState::Undecided)
                && lifted.born == TouchBorn::World
                && survivor.born == TouchBorn::World
            {
                box_select(game, survivor.at, p, false);
            }
            // The survivor is spent EITHER way: after a box
            // or a pinch, its own still release must not
            // read as a tap and select whatever sits under
            // the resting finger.
            input.touches[0].1.spent = true;
            input.pair = None;
        }
        0 => {
            input.pair = None;
            if lifted.still() {
                // A short still touch is a tap: select. Two
                // taps inside the window sweep the kind,
                // like a double-click.
                let double = input.last_tap.is_some_and(|(t, at)| {
                    (input.now - t) * 1000.0 < f64::from(input.touch_prefs.double_tap_ms)
                        && (at - p).length() < click_slop(input.ui) * 2.0
                });
                // Armed modes first, exactly like the
                // mouse: the tap that follows an armed
                // Build or Salvage card completes the
                // command instead of selecting under it.
                // A tap is an atomic click — no drag can
                // follow, so the stroke closes here and
                // Shift decides the mode, like MouseUp.
                if armed_click(game, input, p) {
                    if input.placing_stroke.take().is_some() && !input.resolver.shift_held() {
                        input.placing = None;
                    }
                    input.last_tap = None;
                    return;
                }
                // The minimap owns its taps (jump the
                // camera), and HUD chrome swallows the
                // rest — same ownership order as clicks,
                // or a tap behind the panel would select
                // (and a minimap tap would grab) whatever
                // world ground happens to sit under the
                // chrome pixel.
                if let Some(world) = crate::render::minimap_world_at(&game.view(), p) {
                    game.presentation.camera.center = world;
                    game.presentation.camera.pan(Vec2::ZERO); // re-clamp
                    return;
                }
                // Chrome next, through the touch pad: a
                // fingertip needs 44 logical px even where
                // the drawn card is smaller. A finger that
                // landed on another card activates nothing.
                let layout = game.presentation.layout.get();
                let card = pressed_card(game, p, input.ui);
                let badge = layout.idle_badge;
                if let Some(card) = card {
                    if lifted.card == Some(card) {
                        press_card(game, input, card.hit);
                    }
                } else if badge.w > 0.0 && crate::layout::touch_pad(badge, input.ui).contains(p) {
                    // The idle badge cycles workers by
                    // fingertip too — it sits in the top
                    // bar, which the bare-chrome swallow
                    // below would otherwise eat.
                    cycle_idle_worker(game);
                } else if layout.menu_button.w > 0.0
                    && crate::layout::touch_pad(layout.menu_button, input.ui).contains(p)
                {
                    // Checked before the status so the
                    // menu wins where the padded targets
                    // overlap.
                    input.menu_requested = true;
                } else if layout.pause_status.w > 0.0
                    && crate::layout::touch_pad(layout.pause_status, input.ui).contains(p)
                {
                    dispatch_action(game, input, Action::TogglePause);
                } else if click_on_hud(game, p) {
                    // Bare chrome: the tap is swallowed.
                } else if double {
                    select_all_of_kind_on_screen(game, p, input.ui);
                    input.last_tap = None;
                } else {
                    click_select(game, p, false, input.ui);
                    input.last_tap = Some((input.now, p));
                }
            }
        }
        _ => {}
    }
}

/// The long-press carrier: a held finger emits no events, so its timer
/// rides the frame loop beside `update_held`. A single still touch
/// past the window fires the context gesture ONCE — on an entity it
/// inspects (tap-select), on ground it issues the context order for
/// the current selection, exactly like a right-click.
pub fn update_touch(game: &mut Game, input: &mut InputState) {
    // Chrome owns its ground for the held finger too: a long-press on
    // the minimap or panel band must not order the army to the world
    // point hiding under the HUD. A chrome finger is never spent, so
    // lifting it after reading a card's preview still activates it.
    let Some(tp) = world_hold(input) else {
        return;
    };
    if (input.now - tp.down_at) * 1000.0 < f64::from(input.touch_prefs.long_press_ms) {
        return;
    }
    input.touches[0].1.spent = true;
    let world = game.presentation.camera.to_world(tp.at);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    // Only entities the viewer can actually SEE steer the gesture — an
    // omniscient probe here let a hidden hostile under the fog flip a
    // rally into a select, making occupancy observable through touch.
    let sees = |t: TilePos| game.presentation.all_seeing() || game.my_vision().visible(t);
    let on_entity = game.state.units().iter().any(|u| {
        let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        p.distance(world) <= unit_pick_radius(u.kind)
            && (u.player == game.presentation.human || sees(u.tile()))
    }) || game.state.buildings_at(tile).any(|b| {
        // Same rule as fog, for stealth: an undetected buried charge
        // must not flip a rally into a select, or taps would scan for
        // occupancy the fog view denies.
        b.player == game.presentation.human
            || (sees(tile) && game.state.building_apparent(game.presentation.human, b))
    });
    if on_entity && game.presentation.selection.units.is_empty() {
        select::click_select(game, tp.at, false, input.ui);
    } else {
        orders::context_order(game, tp.at, false);
    }
}
