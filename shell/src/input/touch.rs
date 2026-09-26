//! Gameplay touch gestures: tap, long-press, one-finger pan, the
//! two-finger pinch and box, and the ring that charges under a resting
//! finger. Every finger arrives through the same semantic event funnel
//! as the mouse, in logical pixels.

use super::*;

/// One live finger on the screen.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TouchPoint {
    /// Where it landed.
    pub origin: Vec2,
    /// Where it is now.
    pub at: Vec2,
    /// Wall clock at touch-down (the injected `now`).
    pub down_at: f64,
    /// Whether it ever left the slop circle — a moved finger is a
    /// drag, never a tap or a long-press.
    pub moved: bool,
    /// Whether it LANDED on chrome (minimap or HUD). Chrome-born
    /// fingers never drive world gestures: a swipe starting on the
    /// command panel must not pan the camera behind it, and a
    /// two-finger box with a chrome-born corner must not select.
    pub chrome: bool,
    /// Whether its long-press already fired (fire once per touch).
    pub fired: bool,
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

/// How long a finger must rest before it reads as deliberate rather
/// than the start of a tap, so feedback never flashes under quick taps.
pub(crate) const TOUCH_REST_MS: f64 = 120.0;

/// Where a battlefield long-press is charging and how full it is, from
/// zero once the finger has rested to one as the order fires. Only a
/// lone world-born finger that has neither moved nor fired charges.
pub(crate) fn long_press_progress(input: &InputState) -> Option<(Vec2, f32)> {
    let [(_, finger)] = input.touches.as_slice() else {
        return None;
    };
    if finger.moved || finger.fired || finger.chrome {
        return None;
    }
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
    input.touches.retain(|(tid, _)| *tid != id);
    let chrome =
        crate::render::minimap_world_at(&game.view(), p).is_some() || click_on_hud(game, p);
    input.touches.push((
        id,
        TouchPoint {
            origin: p,
            at: p,
            down_at: input.now,
            moved: false,
            fired: false,
            chrome,
            card: pressed_card(game, p, input.ui),
        },
    ));
    if input.touches.len() > 2 {
        // Three fingers mean nothing yet; the oldest yields.
        input.touches.remove(0);
    }
    if input.touches.len() == 2 {
        // A fresh pair starts undecided, whatever the last
        // pair was doing — a pinch must not outlive its
        // fingers and swallow the next pair's box.
        input.pinching = false;
        input.pair_dist = Some((input.touches[0].1.at - input.touches[1].1.at).length());
    } else {
        input.pair_dist = None;
    }
}

/// A finger moved.
pub(super) fn moved(game: &mut Game, input: &mut InputState, id: u64, p: Vec2) {
    let slop = click_slop(input.ui) * 2.0;
    let two = input.touches.len() == 2;
    let old_dist = two.then(|| (input.touches[0].1.at - input.touches[1].1.at).length());
    let mut delta = Vec2::ZERO;
    if let Some((_, tp)) = input.touches.iter_mut().find(|(tid, _)| *tid == id) {
        delta = p - tp.at;
        tp.at = p;
        if (p - tp.origin).length() > slop {
            tp.moved = true;
        }
    }
    match input.touches.len() {
        // One moved finger drags the world under the hand —
        // unless it landed on chrome, whose ground it keeps.
        1 if input.touches[0].1.moved && !input.touches[0].1.chrome => {
            game.presentation.camera.center -= delta / game.presentation.camera.zoom;
            game.presentation.camera.pan(Vec2::ZERO); // re-clamp
        }
        // Two fingers: a spread that has CUMULATIVELY moved
        // past the threshold is a pinch (zoom at the
        // midpoint) — per-event deltas would miss a slow
        // pinch entirely and mis-commit it as a box select.
        2 => {
            let new_dist = (input.touches[0].1.at - input.touches[1].1.at).length();
            if !input.pinching
                && let Some(start) = input.pair_dist
                && (new_dist - start).abs() > crate::viewer_touch::PINCH_START_PX * input.ui
            {
                input.pinching = true;
            }
            if input.pinching
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
        return;
    };
    let (_, lifted) = input.touches.remove(pos);
    match input.touches.len() {
        // Second finger of a pair released: a pair that
        // never pinched commits the box between the fingers
        // — both corners world-born; a chrome-born finger
        // boxes nothing behind its panel.
        1 => {
            if !input.pinching && !lifted.chrome && !input.touches[0].1.chrome {
                let other = input.touches[0].1.at;
                box_select(game, other, p, false);
            }
            // The survivor is spent EITHER way: after a box
            // or a pinch, its own still release must not
            // read as a tap and select whatever sits under
            // the resting finger.
            input.touches[0].1.moved = true;
        }
        0 => {
            input.pinching = false;
            input.pair_dist = None;
            if !lifted.moved && !lifted.fired {
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
    if input.touches.len() != 1 {
        return;
    }
    let (_, tp) = input.touches[0];
    if tp.moved || tp.fired {
        return;
    }
    if (input.now - tp.down_at) * 1000.0 < f64::from(input.touch_prefs.long_press_ms) {
        return;
    }
    // Chrome owns its ground for the held finger too: a long-press on
    // the minimap or panel band must not order the army to the world
    // point hiding under the HUD. The finger stays unfired, so lifting
    // it after reading a card's preview still activates the card.
    if crate::render::minimap_world_at(&game.view(), tp.at).is_some() || click_on_hud(game, tp.at) {
        return;
    }
    input.touches[0].1.fired = true;
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
