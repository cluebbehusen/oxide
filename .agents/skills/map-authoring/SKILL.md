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
