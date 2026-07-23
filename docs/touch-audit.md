# Touch audit (0.10 Phase D)

Inventory of every interaction that assumes a mouse, with a verdict
for a future touch shell. The protocol already carries touch RawEvents;
this is the shell-side half. Verdicts: OK (works as taps/drags today),
SHIM (needs a touch-specific translation, listed), BLOCKER (needs a
real alternative before any touch shell ships).

## Pointer verbs

| Interaction | Today | Verdict |
|---|---|---|
| Select unit / building | LMB tap | OK — tap |
| Drag-select box | LMB drag past threshold | OK — drag; threshold already ui-scaled (`drag_threshold`) |
| Command (move/engage/harvest) | RMB | BLOCKER — no right button. Shim: long-press = command, or a tap-mode toggle button in the chrome. Decision deferred to the touch shell. |
| Queue order | Shift + RMB | BLOCKER — modifier chord. Shim: a queue-mode latch button beside the panel. |
| Minimap jump / steer | LMB press + drag | OK — tap and drag |
| Camera pan | MMB drag / edge pan / arrows | SHIM — two-finger drag is the natural mapping; protocol touch events carry multi-touch. |
| Zoom | wheel | SHIM — pinch; wheel deltas already normalized. |
| Scrub bar | LMB press + drag | OK |
| Panel cards / queue ghosts | LMB tap | OK — cards are 66x80 logical, comfortably above the ~44px touch minimum at 1x. |
| Build placement | LMB tap to commit, Esc cancels | SHIM — needs an on-screen cancel affordance (the toast names Esc). |
| Delete replay | X key twice | SHIM — the two-press arming maps to two taps if a delete affordance is drawn; today it is keyboard-only. |

## Hover-only information (invisible to touch)

| Surface | Today | Verdict |
|---|---|---|
| Card tooltips (what it is, cost, weapons, hotkey) | hover | BLOCKER for parity — press-and-hold should raise the same tooltip. The panel model already carries all the data; the gap is a hold-timer in input. |
| Salvage amounts under cursor | hover | SHIM — same press-and-hold surface. |
| Menu row hover highlight | hover | OK to lose — selection highlight remains. |
| Map-list preview follows hover | hover, falls back to keyboard cursor | OK — falls back to the selected row on touch. |

## Timings

Double-click (unit-type select) and double-tap (group recall
centering) read `input.now` with fixed windows — both already run on
injected wall clock, so a touch shell can widen them; making the
window a config knob is a one-liner when needed.

## Bottom line

Nothing structural blocks a touch shell: the funnel is semantic, every
geometry source is shared, and the two real BLOCKERs (the RMB command
verb and hover tooltips) both have clear shims — a command long-press
or mode latch, and press-and-hold tooltips. Neither shim is built in
0.10; this audit is the contract for the shell that needs them.
