---
name: map-authoring
description:
  Create, edit, audit, and validate Oxide scenario maps and their player setups.
  Use for scenario JSON, terrain layout, quarry boundaries, pits, Extractor
  frames, scrap placement, Foundry anchors, teams, spawn rosters, map metadata,
  symmetry, route fairness, artillery pressure, or shipped-map gate failures.
---

# Oxide map authoring

Author a map as a gameplay space first. Use mechanical checks for terrain,
symmetry, and seat fairness, then play it to judge routes, pressure, and pacing.

## Use the scenario language exactly

Confirm the live parser in `sim/src/map.rs` and schema in `sim/src/scenario.rs`.
The current authorable bytes are:

- `.`: passable ground;
- `,`: cosmetic rubble on ordinary ground;
- `#`: rock, which blocks ground and direct ground fire;
- `^`: peak, which blocks ground, air, and every shot across it;
- `~`: pit, which blocks ground while air and fire cross;
- `s` / `S`: normal / rich scrap;
- `E`: top-left tile of a derelict 2x2 Extractor frame;
- `1`–`8`: top-left Foundry anchors for seats 1–8;
- `a`–`h`: top-left Foundry anchors for seats 9–16.

Do not author `w`; it is a rendered wreck marker. Keep every row the same width.
An `E` needs a clear 2x2 ground footprint and represents one fixed build site,
not four independent tiles.

Fill maintained browser metadata: `hook`, `pace`, `mode`, `richness`, and
`theme`. Accepted pace classes are `quick`, `standard`, `large`, `vast`, and
`grand`; richness is `lean`, `standard`, or `rich`. Treat `duration` as an
optional measured claim, not an estimate from dimensions. Add it only after the
current opponent and real play have produced evidence worth publishing.

## Choose the fairness class

The default, empty `meta.symmetry` value claims exact 180-degree authoring. For
a tile `(x, y)` on a `width` by `height` map, its image is
`(width - 1 - x, height - 1 - y)`. Rotate a footprint with top-left `(x, y)` and
size `(w, h)` to `(width - w - x, height - h - y)`.

Derive paired seats from rotated Foundry footprints, never from seat numbers.
The relation must be an involution and pair hostile seats. Each pair needs:

- equal starting scrap;
- role-equivalent mirrored unit lists in stable order;
- mirrored prebuilt structures using their real footprint dimensions;
- equal access to scrap and Extractor frames.

Use `meta.symmetry: "metric"` only for intentionally non-mirrored layouts such
as free-for-alls. The audit then proves comparable room, routes, resources, and
frame access within the maintained tolerances instead of claiming identical
tiles.

An omitted team means a one-seat team. Never place everyone on one team. Shipped
content keeps seat 0 human with no bot config; every other seat has `bot: true`
and `bot_config: {"controller": "scripted"}`. Default factions alternate Ferrous
and Cupric; launch-time retinting handles player choices.

## Design for decisions

Give every reachable region a reason to exist: salvage, a route, an expansion,
high ground pressure, a safe staging area, or a defensible approach. Avoid
decorative dead acreage and layouts where one uncontestable opening claim
decides the economy.

Check these questions while authoring:

- Can every seat fund a viable opening from nearby salvage?
- Is there a reason to leave the starting pocket?
- Are alternate routes meaningfully different rather than cosmetic twins?
- Is there room to build where armies and harvest lines actually travel?
- Can a losing player contest new value, or does the winner own every recovery
  path after one fight?
- Do terrain silhouettes make each movement and fire rule legible?
- On island maps, can the bot and player reach air or transport play before
  their ground economy deadlocks?

Cosmetic scenery stays separate from hashed terrain unless it deliberately
changes gameplay. Use the visual-assets workflow for quarry dressing and edge
art.

## Audit while iterating

Run the exact map audit from the repository root:

```sh
cargo run -q -p oxide-driver -- map-audit scenarios/<map>.json
cargo run -q -p oxide-driver -- map-audit scenarios/<map>.json --json
```

Inspect reachable room, nearest scrap, hostile ground and air routes, artillery
pressure, spawn spacing, pairings, and Extractor access. The maintained bands
and tolerances in `driver/tests/map_gates.rs` are authoritative; do not
duplicate their current numbers in this skill.

Then run the shipped gates and a real liveness pass:

```sh
cargo test -p oxide-driver --test map_gates --locked
cargo test -p oxide-driver --test headless --locked
cargo run -q -p oxide-driver -- run scenarios/<map>.json --ticks 12000 --all-bots --map
```

Use the longer window already established by the tests for vast and grand maps.
Inspect the replay summary and watch representative games. A connected,
symmetric, decisive map can still be dull or confusing.

## Keep drafts out of the shipped pool

Draft in `map-drafts/`, never directly in `scenarios/`. The shipped-map sweep
and hash blessing read the entire scenarios directory.

Generate the local review page with:

```sh
uv run tools/map_review.py
```

When changing the review-page builder, run its orchestration and escaping tests:

```sh
uv run --python 3.14 -m unittest tools.test_map_review
```

The review page presents drafts; it never promotes them. Moving a draft into
`scenarios/`, blessing hashes, and committing it require explicit user approval.
Inspect every changed hash and golden after an intended promotion.
