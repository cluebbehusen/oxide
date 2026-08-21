---
name: visual-assets
description: Generate, modify, review, animate, and productionize Oxide visual assets. Use for sprites, action animations, projectiles, construction states, environment props, scenery, decals, rocks, debris, terrain, quarry boundaries, UI icons, or changes to tools/gen_sprites.py, tools/production_sprite_sources, and assets/sprites. This skill keeps role readability and explicit approvals ahead of visual novelty.
---

# Oxide visual assets

Use this workflow whenever creating or changing Oxide's code-generated lo-fi
pixel art. The goal is a coherent family whose individual machines and terrain
semantics remain recognizable at actual battlefield scale.

## Establish the control

1. Read `docs/visual-approvals.md`, the relevant production generator code,
   animation trigger code, and current in-game asset before designing.
2. If Connor called an asset good, close, or finalized, copy that exact design
   into the next comparison as a control. Never recreate it from memory.
3. Keep experiments, review batches, capture harnesses, and source mockups
   untracked. Do not delete, ignore, move, or commit them during promotion.
4. Separate static design questions from motion questions. Units, buildings,
   projectiles, and other moving assets need both a static image and separately
   labeled animation. Static props, terrain, decals, and icons need the static
   asset plus native-context review, not a fabricated loop.

## World and visual language

Oxide battles take place on the floors of exhausted open-pit quarries on an
abandoned mining world. The former rush was industrial and once prosperous;
what remains is lonely, faintly eerie, and still mechanically busy. Terraces
rise away from the battlefield into darkness, while rocks, obsolete equipment,
and mining debris break up the quarry floor.

- Work in charcoal, oxidized iron, rust, patina, faded corporate paint, and
  restrained hazard accents. Avoid cheerful saturation and generic neon sci-fi.
- Use bold silhouettes, selective highlights, and a full-strength light rim for
  gameplay contrast.
- Give each unit or building one unmistakable role feature: tuning fork,
  artillery spade, spider legs, drill, hopper, open ring, transfer hall, or
  another legible mechanism.
- Keep family resemblance in palette, pixel density, outline weight, and
  material treatment. Create identity through physical form, not a wholly new
  rendering style for each asset.
- Preserve terrain semantics visually. Passable debris stays low; ground
  blockers have mass; tall quarry remnants communicate that they also block air
  and fire.

## Explore without losing the product

For a new or open visual direction, use ImageGen or a similar image-generation
tool — if your session actually has one — to explore silhouettes, mechanisms,
and material ideas before authoring candidates. Check your tool registry
rather than assuming: Codex sessions have ImageGen; Claude sessions currently
do not. Without such a tool, do the same exploration by authoring materially
distinct silhouette archetypes directly in code — vary the body plan and
mechanism per option, not the palette. Image generation is not required when
promoting an already approved design or making a tightly bounded edit whose
direction is already fixed. Use generated imagery as reference, not as the
shipping raster. Reduce the useful idea into Oxide's authored pixel geometry
and production palette.

When direction is open, generate several materially distinct options in an
untracked batch script or temporary review workspace. Do not accumulate
one-off review routes in the production generator. When a design is close,
change only the requested identity or mechanism and keep the control beside it.
Avoid decorative complexity, duplicated recoil, extra weapon reports, or a
redesign that discards already approved structure.

Review each candidate in all relevant contexts:

- isolated static at nearest-neighbor enlargement;
- animated without interpolation;
- native gameplay scale on the real floor palette;
- faction colors and full rim;
- air shadow or other role context when silhouette alone is misleading;
- construction, selection, fog, damage, projectile, and orientation states as
  applicable.

## Animate actions

Animate mechanisms, not generic overlays: grab, grind, pull, press, weld, feed,
recoil, reload, settle, tread, hover, or rotate. Two to four strong poses usually
read better than many subtle frames. Use anticipation, decisive motion, and
slower recovery to convey weight.

Wire motion to the action that causes it. Buildings animate only while they are
actually constructing or repairing; weapons recoil only when firing; charge or
cargo indicators reflect their real state. Array and Reclaimer may loop because
their operation is continuous. Projectile count, launch point, timing, and
damage reports must agree with the animation.

## Approve and promote

1. Record the source/control SHA-256 and provenance, then present numbered
   options with the exact control included. Include animation only for assets
   whose role actually moves.
2. Record only Connor's explicit approvals in `docs/visual-approvals.md`.
3. Promote only those approved bytes or their exact code-native source through
   `tools/gen_sprites.py` and, where used, `tools/production_sprite_sources/`.
   Assert that regenerated production bytes match the approved source. Do not
   promote an entire review batch because one item was approved.
4. Keep the production generator, atlas outputs, runtime wiring, tests, and
   narrow approval ledger in one reviewable change. Commit them together only
   when the user has explicitly authorized a commit. Leave every experiment in
   place and uncommitted until a separate cleanup is requested.
5. Inspect the dirty tree and changed atlas keys before staging. Stage exact
   production paths rather than `git add -A`; verify no review generator,
   capture harness, or alternate asset entered the index.

## Validate before handoff

Run the generator tests and `--check`, atlas-key bijection tests, relevant Rust
rendering tests, and any native capture harness needed for normal-scale visual
inspection. Inspect the regenerated atlas and actual GPU shell, not just review
cards or the CPU schematic renderer. Check every map shape when changing quarry
boundaries. Finish with repository-wide tests, Clippy, and format checks from
`AGENTS.md`.
