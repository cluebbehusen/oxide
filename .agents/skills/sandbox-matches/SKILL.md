---
name: sandbox-matches
description:
  Stage throwaway Oxide sandbox scenarios with preset armies, prebuilt
  structures, and a passive or absent enemy. Use when the user wants a drill or
  practice match against a non-playing opponent, or when a task needs an exact
  staged scene for screenshot or animation validation, isolated combat or
  mechanic repros, or quick unit-versus-structure probes.
---

# Oxide sandbox matches

A sandbox stages chosen units and structures without requiring an opponent,
Foundries, or a victory condition. It uses ordinary simulation commands and
rules, so saves and replays reproduce the scene. A sandbox proves mechanics and
presentation, not opponent credibility; use the scripted-bot skill for that.
Keep throwaway scenes outside the shipped scenario pool.

## Build the scenario

Use the ordinary scenario schema in `sim/src/scenario.rs`, with
`"mode": "sandbox"`. This makes Foundry anchors optional, permits allied-only or
disconnected scenes, and disables automatic elimination and victory. It does not
bypass terrain, placement, ownership, cost, or command validation.

- At least one player seat is required because units need an owner. A seat does
  not require a human or a bot controller.
- Use `"bot": false` for a passive seat. A `"bot": true` seat without
  `bot_config` is also an empty chair. A configured bot runs normally; the
  strategic controller currently needs a Foundry to act, so omit it when only
  staging units. Units and turrets still auto-defend without a controller.
- Local input controls the first non-bot seat, falling back to seat zero if
  every seat is bot-controlled. This does not disable any configured bot. Debug
  commands explicitly identify their seat.
- `units` places starting units at tile coordinates. Walkers need open ground;
  flyers may start over terrain they can legally hover over.
- `buildings` places completed structures by top-left anchor. Their full
  footprint must fit passable ground without overlapping another structure.
- Optional map anchor bytes place Foundries. Omit them for a unit-only scene.
  Without sandbox mode, ordinary Foundry and victory requirements still apply.

A minimal unit-only sandbox:

```json
{
  "name": "Movement Drill",
  "mode": "sandbox",
  "seed": 1,
  "map": [
    "....................",
    "....................",
    "....................",
    "....................",
    "....................",
    "....................",
    "....................",
    "...................."
  ],
  "players": [
    { "name": "Local", "faction": "ferrous", "scrap": 0, "bot": false }
  ],
  "units": [{ "player": 0, "kind": "scuttler", "x": 5, "y": 4 }]
}
```

Add passive seats and enemy entities when needed. Sandbox combat continues
without declaring a winner after a side loses its units or buildings. Explicit
surrender still relinquishes command authority for that seat. Repeat unit specs
at distinct standable tiles for larger scenes; kind names come from
`sim/src/stats.rs`.

## Stage a manual art review

Default human review scenes to a spacious proving ground, around 80 by 52 tiles,
with units spread out and enemy targets across a wide empty approach. Start
paused with no opening commands. A passive opponent still auto-defends, so check
sight and weapon ranges and run an idle interval to prove there is no opening
combat. Preserve normal aircraft idle flight.

Keep several previously reviewed units for comparison and enemy buildings for
manual target practice. Frame the friendly staging area at a readable zoom;
allow the player to move one unit into range at a time. Keep close-range
scripted firing and animation diagnostics in separate replays, and return the
final review window to the quiet scene before handing it over.

## Launch it

The shell takes a scenario path directly and skips the menu:

```sh
cargo run -p oxide-shell -- --scenario path/to/sandbox.json
```

Every driver entry point that takes a scenario accepts a sandbox the same way —
`run <path>` for headless ticks, `render <path> --out <png>` for CPU schematic
captures, `session --scenario <path>` for the windowless debug session — and the
oxide-live-qa skill drives the live shell against one for real-window
screenshots, input, and animation checks.

`Scenario::load` is a plain file read, so a scenario can be fed inline from zsh
with process substitution instead of a saved file:

```sh
cargo run -p oxide-shell -- --scenario <(cat <<'EOF'
{ ...scenario JSON... }
EOF
)
```

## Keep it out of the shipped pool

Never place a sandbox in `scenarios/`, even uncommitted: the shipped-map gates,
hash sweep, and browser read the entire directory. Keep sandboxes in scratch
space or `map-drafts/`, and delete them when the task ends. Sandboxes are exempt
from the shipped-map expectations — metadata badges, symmetry claims, audits,
and pacing judgments all stay in the map-authoring workflow.
