---
name: map-authoring
description: Create, edit, audit, and validate Oxide scenario maps and their player setups. Use for scenario JSON, terrain layout, quarry boundaries, scrap placement, Foundry anchors, teams, spawn rosters, map metadata, symmetry, route fairness, artillery pressure, or shipped-map gate failures.
---

# Oxide map authoring

Author maps as gameplay spaces first. Preserve the shipped symmetry and seat
fairness contracts mechanically instead of relying on visual resemblance.

## Read the scenario language

Use these map bytes only:

- `.`: passable ground.
- `,`: cosmetic rubble on otherwise ordinary ground.
- `#`: rock; blocks ground movement but not air.
- `^`: connected uncut-quarry mesa; blocks ground, air, and fire.
- `s`: ordinary scrap node.
- `S`: rich scrap node with double salvage.
- `1` through `8`: the top-left anchor of that seat's 2x2 Foundry.

Do not author `w`; it appears only when rendered ground contains a wreck.
Keep every row the same width. Confirm parser behavior in `sim/src/map.rs` and
scenario schema in `sim/src/scenario.rs` rather than inventing new bytes or
fields.

Fill shipped metadata completely: `hook`, `pace`, `duration` for 1v1 maps,
`mode`, `richness`, and `theme`. Use only the gated `pace` values `quick`,
`standard`, `large`, or `vast`, and `richness` values `lean`, `standard`, or
`rich`. Existing themes are `basalt`, `cold-circuitry`, `quarry-dust`,
`rusted-yard`, `slag`, and `verdigris`; confirm the live scenario catalog
before adding one. Derive duration from measurement rather than dimensions.

## Preserve seats and symmetry

Keep shipped terrain exactly 180-degree symmetric, including rubble and scrap
amounts. For a tile at `(x, y)` on a `width` by `height` map, place its image at
`(width - 1 - x, height - 1 - y)`.

Derive seat partners from rotated Foundry footprints, never from seat numbers.
For a building with top-left anchor `(x, y)` and footprint `(w, h)`, place its
image at `(map_width - w - x, map_height - h - y)`. The partner relation must
be an involution and must pair hostile seats.

For each paired seat:

- Give equal starting scrap.
- Mirror units entry by entry at tile coordinates. Compare faction-varied kinds
  by `Role`, not literal kind.
- Mirror prebuilt structures entry by entry using their footprint dimensions.
- Keep every authored starting unit list grouped and ordered consistently;
  entity ids inherit list order and simulation tie-breaks inherit ids.

Treat omitted `PlayerSpec.team` as a distinct one-seat team. Do not assign every
seat to one team. Keep seat 0 human and every other shipped seat bot-enabled.
Use even-seat Ferrous and odd-seat Cupric as the authored default; launch-time
retinting handles player choices.

For 6- and 8-seat lane maps, build identical self-symmetric lanes and require
strict scrap equality across every seat. For 4-seat maps, inspect the partner
relation from anchors rather than assuming either possible pairing.

## Design for play

Give every reachable region a reason to exist through resources, routes,
positioning, expansion pressure, or defensible geometry. Avoid decorative dead
corners and scrap layouts that make one early claim irrecoverably decide the
economy. Keep air and ground routes intentional, leave room outside artillery
coverage, and check that blockers communicate their movement domain.

Use quarry scenery and boundaries from the visual-assets workflow, but keep
cosmetic props separate from hashed terrain unless they intentionally affect
gameplay. Make map edits in mirrored pairs as they are authored.

## Audit before accepting

Run the single-map audit during iteration:

```sh
cargo run -q -p oxide-driver -- map-audit scenarios/<map>.json
cargo run -q -p oxide-driver -- map-audit scenarios/<map>.json --json
```

Inspect reachable room, nearest scrap, hostile ground and air routes,
artillery pressure, and spawn spacing. Route labels are gated against the
simulation's weighted movement costs, including diagonal no-corner-cutting.
The current hostile-route bands are quick 8–28, standard 29–52, large 53–90,
and vast 91–150. Quick-map artillery pressure is capped at 0.65 and every other
pace at 0.50; `driver/tests/map_gates.rs` is authoritative if these values move.

Then run the map gates and a real liveness pass:

```sh
cargo test -p oxide-driver --test map_gates --locked
cargo test -p oxide-driver --test headless --locked
cargo run -q -p oxide-driver -- run scenarios/<map>.json --ticks 12000 --bots --map
```

Use the longer shipped-map tick window for vast maps when the existing gate
does. Inspect human play and bot play; symmetry is necessary but cannot prove
that lanes, corners, pressure, or economy are interesting.

If an intended map edit moves a hash fixture, follow the root version-and-bless
contract. Inspect changed goldens and hashes before staging. Additions and
removals must still leave the complete shipped-map sweep green.

## Stage drafts outside the shipped pool

Never author a draft directly in `scenarios/` — the shipped-map gates,
hash fixtures, and bless sweeps read that directory, so an unblessed
draft pollutes fixtures the moment anything blesses. Drafts live in
`map-drafts/` until the user approves them.

Present drafts for review with the review page, then open (or send)
the single self-contained HTML it writes:

```sh
uv run tools/map_review.py            # renders + audits + mirror probe
uv run tools/map_review.py --no-probe # skip the slow mirror probe
open map-review/index.html
```

Blessing a draft is explicit and user-driven: move its JSON into
`scenarios/`, re-run the map gates and headless sweep, bless the hash
fixture row, and commit. The review page presents; it never promotes.

Seat a real opponent in every draft: `"bot": true` alone seats nobody —
the actor requires `"bot_config": {"level": ...}`, and a draft without
it plays its liveness runs against an idle seat.

## Measured design lessons (0.15 pool health pass)

- The opening economy must not starve before the first push outward:
  give each start enough near pods to fund the opening, and
  stepping-stone clusters toward the far ring. Lean near economies
  produced 40k-tick mutual stalls between full-strength mirrors.
- A dominant central prize is what buys perturbation robustness. On
  maps whose mid-game war is worth more than an opening tempo nudge,
  a forced early Turret flips nothing; on open maps without one it
  flipped every baseline (subsidence, basalt-spine, the-deep-cut).
  Measure with the turret control probe and read the flip counts:

  ```sh
  cargo run -q -p oxide-driver --release -- viability-probe \
      --weights sim/src/bot/ladder_weights.json \
      --scenario map-drafts/<draft>.json --action turret --quota 1
  ```

- Every decisive expert mirror in the pool is a fixed-seat sweep (one
  chair wins every seed; which chair varies by map). Do not gate on
  seed-varying winners — it does not exist. Gate on decisive mirrors
  plus perturbation robustness, and read the probe's baseline seat
  split as information, not a pass/fail.
- Acreage is not openness: a similar-sized map can block nearly half
  of all builder thinks (cinder-steppe) while another expresses every
  structure. Leave genuine room to build along the routes players
  actually take.
- The `grand` pace class (151-400 effective steps) exists for island
  wars and campaign-length maps; `~` gulf severs ground while air and
  fire cross, and the route gate accepts air-only hostile pairs. Maps
  above vast scale belong there.
