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

A sandbox is a throwaway scenario that stages an exact scene: chosen units and
structures at chosen positions, against an opponent that never acts. Reach for
it when competitive fairness would only get in the way — a user practice drill,
a staged scene for screenshots or native animation capture, an isolated combat
or mechanic repro, or a quick "ten of X versus one Y" probe.

A sandbox proves mechanics and presentation, not opponent credibility. Bot
behavior judgments go through the scripted-bot skill's evaluation paths, and a
sandbox never ships: promotion into `scenarios/` means authoring a real map
under the map-authoring skill instead.

## Build the scenario

Sandboxes use the ordinary scenario schema in `sim/src/scenario.rs` and the
terrain bytes documented in the map-authoring skill. The sandbox-specific moves:

- A passive seat is `"bot": true` with no `bot_config`: a documented empty chair
  (`seat_bots` in `sim/src/bot/mod.rs`) that never issues commands, so it cannot
  build, train, harvest, or maneuver — but its units and turrets still
  auto-defend, because return fire is simulation behavior, not controller
  behavior. Never mark a passive seat `"bot": false`: the shell's playable
  session demands exactly one non-bot seat and refuses to launch with two
  humans.
- `units` places starting units per seat at tile coordinates. Walkers need open
  ground; flyers may legally start over any tile they could hover over in play.
- `buildings` places completed structures beyond the Foundries. Coordinates are
  the top-left anchor; the full footprint must fit passable ground, and overlaps
  are rejected.

Validation still enforces the invariants that make the match runnable, even in a
sandbox:

- Every seat needs its Foundry anchor byte in the map text
  (`ScenarioError::MissingAnchor`), so a passive seat always owns at least a
  Foundry.
- With two or more players, at least two hostile teams must exist
  (`ScenarioError::OneTeam`). A one-player scenario is legal and never resolves,
  so a truly enemy-free stage — pure scenery, movement, or screenshot work —
  needs no opponent seat at all.
- Every pair of Foundries must share a route some mover can take.

A team with no Foundries left is eliminated, and a lone surviving team wins.
Make the passive seat's Foundry the drill's victory target, or keep it away from
the action when the scene must outlive the fight.

A verified minimal sandbox — one human seat with a Scuttler against a passive
seat holding a Bastion:

```json
{
  "name": "Bastion Drill",
  "seed": 1,
  "map": [
    "....................",
    "....................",
    "..1.............2...",
    "....................",
    "....................",
    "....................",
    "....................",
    "....................",
    "...................."
  ],
  "players": [
    { "name": "You", "faction": "ferrous", "scrap": 150, "bot": false },
    { "name": "Target", "faction": "cupric", "scrap": 0, "bot": true }
  ],
  "units": [{ "player": 0, "kind": "scuttler", "x": 5, "y": 5 }],
  "buildings": [{ "player": 1, "kind": "bastion", "x": 16, "y": 5 }]
}
```

Repeat the unit spec at distinct standable tiles for each extra unit. Kind
strings are the lowercase names from `sim/src/stats.rs`.

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
