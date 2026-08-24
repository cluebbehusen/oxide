---
name: visual-assets
description:
  Generate, modify, review, animate, and productionize Oxide visual assets. Use
  for sprites, action animations, projectiles, construction states, environment
  props, scenery, decals, rocks, debris, terrain, quarry boundaries, UI icons,
  or changes to tools/gen_sprites.py, tools/production_sprite_sources, and
  assets/sprites. This skill keeps role readability and explicit approvals ahead
  of visual novelty.
---

# Oxide visual assets

Use this workflow whenever creating or changing Oxide's code-generated lo-fi
pixel art. The goal is a coherent family whose individual machines and terrain
semantics remain recognizable at actual battlefield scale.

## Establish the control

1. Read `docs/visual-approvals.md`, the relevant production generator code,
   animation trigger code, and the complete current in-game sequence before
   designing. For a new machine family, study the finalized Harvester, Lancer,
   Bombard, Flakhound, Buzzard, Wisp, Foundry, Fabricator, Reclaimer, and Repair
   Bay as applicable, including their movement, action, work, cargo, charge, or
   construction states. A single idle PNG is not a sufficient control.
2. If Connor called an asset good, close, or finalized, copy that exact design
   into the next comparison as a control. Never recreate it from memory.
3. Use finalized art as positive evidence for component construction, material
   layering, purposeful detail, and state storytelling. Reuse the roster's
   design vocabulary such as tread pads, hinges, braces, vents, tool joints,
   recesses, and feed paths without tracing or lightly reshaping an existing
   overall silhouette.
4. Treat rejected review banks and their direction specs as excluded creative
   inputs unless Connor names an exact candidate as a control. A rejected option
   may be a diagnostic clue without becoming a shape donor.
5. Put experiments, review batches, capture harnesses, and source mockups under
   a repository-local path that Git actually ignores. Prove the concrete output
   path with `git check-ignore -v <candidate-path>`; merely leaving files
   untracked is not sufficient. Do not delete, move, or commit them during
   promotion.
6. Separate static design questions from motion questions. Units, buildings,
   projectiles, and other moving assets need both a static image and separately
   labeled animation. Static props, terrain, decals, and icons need the static
   asset plus native-context review, not a fabricated loop.

## World and visual language

Oxide battles take place on the floors of exhausted open-pit quarries on an
abandoned mining world. The former rush was industrial and once prosperous; what
remains is lonely, faintly eerie, and still mechanically busy. Terraces rise
away from the battlefield into darkness, while rocks, obsolete equipment, and
mining debris break up the quarry floor.

- Work in charcoal, oxidized iron, rust, patina, faded corporate paint, and
  restrained hazard accents. Avoid cheerful saturation and generic neon sci-fi.
- Use bold silhouettes, selective highlights, and a full-strength light rim for
  gameplay contrast.
- Give each unit or building one unmistakable role anchor plus two to four
  supporting functional details. At native scale, a reviewer should be able to
  name its locomotion or foundation, tool or weapon, and feed, storage, power,
  or bracing path. A large outlined mass with one symbol is not enough.
- Keep family resemblance in palette, pixel density, outline weight, and
  material treatment as well as shared component language. Create identity
  through the arrangement and operation of those parts, not a wholly new
  rendering style or a lightly altered old silhouette.
- Lo-fi does not mean under-detailed. Use cleats, joints, hoses, shells, cells,
  braces, vents, racks, apertures, fasteners, wear, and layered recesses when
  they explain construction or operation. Avoid purposeless noise, not useful
  mechanical detail.
- Preserve terrain semantics visually. Passable debris stays low; ground
  blockers have mass; tall quarry remnants communicate that they also block air
  and fire.

## Calibrate fidelity before breadth

Do not begin with a whole roster. First finish one representative calibration
for each in-scope class: unit, building, and/or terrain treatment. Stop for
human art-direction approval before multiplying the approach. A larger batch is
not admitted merely because it is complete, deterministic, or quantitatively
distinct.

Judge each calibration candidate at three scales:

- thumbnail: the role and facing survive;
- native gameplay scale: chassis, tool, and supporting parts remain separable;
- enlarged nearest-neighbor: every small detail has a mechanical reason.

The static frame must forecast how the machine works. Before drawing, write a
short causal path such as `treads -> chassis -> shell rack -> breech -> barrel`
or `crane -> hopper -> conveyor -> cargo bay`. Reject silhouettes whose parts
cannot be described as an assembled mechanism. Human fidelity, charm, and role
readability are admission gates; IoU, bbox, fill, profile, and collision metrics
are later diagnostic tripwires, never substitutes for art judgment.

## Explore without losing the product

For a new or open visual direction, use ImageGen or a similar image-generation
tool — if your session actually has one — to explore complete mechanical
assemblies, state changes, and material ideas before authoring candidates. Check
your tool registry rather than assuming: Codex sessions have ImageGen; Claude
sessions currently do not. Prefer one calibration entity per concept study over
a dense whole-roster board. Without such a tool, author materially distinct
mechanical archetypes directly in code. Image generation is not required when
promoting an already approved design or making a tightly bounded edit whose
direction is fixed.

Use generated imagery as reference, not as the shipping raster. Before reducing
it to Oxide geometry, list the dominant mechanism and the purposeful secondary
details worth preserving. After authoring, verify that those details survived;
do not translate a rich machine into topology alone.

When direction is open, generate several materially distinct options in a
gitignored batch script or temporary review workspace. Do not accumulate one-off
review routes in the production generator. When a design is close, change only
the requested identity or mechanism and keep the control beside it. Avoid
purposeless noise, duplicated recoil, extra weapon reports, or a redesign that
discards already approved structure.

Review each candidate in all relevant contexts:

- isolated static at nearest-neighbor enlargement;
- animated without interpolation;
- native gameplay scale on the real floor palette;
- faction colors and full rim;
- air shadow or other role context when silhouette alone is misleading;
- construction, selection, fog, damage, projectile, and orientation states as
  applicable.

## Package review for selection

`tools/asset_review.html` is the primary review surface for candidate banks. Do
not replace it with a custom gallery, a contact sheet, a README full of
thumbnails, or a directory the reviewer must inspect manually.

Build the review package around decisions rather than around generation output:

1. Give every candidate option one stable, unique integer review ID. Prefix its
   filename with the zero-padded ID and make that ID visible in the review card
   title or asset. Never recycle or renumber an ID while that review remains
   active, including after adding or withdrawing candidates.
2. Make each candidate a separately navigable item in `tools/asset_review.html`.
   One option must not be a cell hidden inside a multi-option contact sheet. If
   a static and animated view support the same decision, keep one clearly
   identified primary candidate in the session and put additional views in
   evidence keyed to the same ID.
3. Use small, coherent session directories. Normally create one session per unit
   or building, containing only that entity's alternative directions; use a
   separate session for a terrain question such as pits. Do not make the
   reviewer scan an entire roster to compare one entity.
4. Keep session directories free of contact sheets, atlases, proof mosaics,
   source images, and generation intermediates because the review tool loads
   media recursively. Put those in sibling `evidence/` or `sources/`
   directories, keyed by the same stable IDs. They support the decision but are
   not the selection UI.
5. Maintain a concise ledger mapping each ID to entity, direction, primary
   filename, session, and evidence. The filename ID is authoritative, and the
   review tool must display that same ID even when a small session is opened by
   itself.

For pits and quarry drops, each numbered option must be a genuinely different
structural system: for example, a sheer highwall, terraced cut, collapsed face,
retained working, or abandoned lower level. Do not spend separate IDs on the
same topology with changed scratches, speckles, or edge noise. The production
black void is a negative control, not a style control: a pit is a visibly lower,
terrestrial quarry level with a rock face, cliff foot, and textured floor.

Place an exact finalized tracked unit beside the lip in review evidence. Review
at default gameplay zoom and full-map zoom, and reject any drop that looks
smaller than its treads or plausibly driveable. Establish the desired physical
scale in unconstrained concept art before forcing it into the current autotile
renderer. If the approved depth needs multi-tile faces, overdraw, distance
fields, or region-scale shadows, report that renderer requirement instead of
flattening the design to fit existing tiles. Give every pit ID a full-map
primary view plus native-scale edges, corners, junctions, and traversal context
in sibling evidence.

At handoff, give exact launch and selection instructions. From the repository
root, the standard launch is:

```sh
open tools/asset_review.html
```

Then tell the reviewer to click **Choose asset directory** and name each exact
session directory to select, together with its ID range. Do not merely say to
open the review root. If the native directory picker is unavailable, use the
page's compatibility picker in Chrome or Edge. Open each named session in the
tool yourself before handoff and verify that every card is one intended
candidate, sorted by its stable ID, with no supporting artifact changing the
card count.

## Animate actions

Animate mechanisms, not generic overlays: grab, grind, pull, press, weld, feed,
recoil, reload, settle, tread, hover, or rotate. Give applicable assets a
gameplay-state story as well: cargo fills, ammunition stations empty, clamps
close, hoses pay out, capacitors charge, roofs open, or work advances through a
visible process. Two to four strong poses usually read better than many subtle
frames for one action; they are not a cap on persistent cargo, charge, payload,
damage, or construction states. Use anticipation, decisive motion, and slower
recovery to convey weight.

During direction exploration, concept-only action or state animation is allowed
when it is clearly labeled `requires runtime wiring`; this lets the art propose
the repair, mining, loading, or unloading behavior being reviewed. Promotion
requires honest integration: buildings animate only while actually working;
weapons recoil only when firing; charge, cargo, and payload indicators reflect
real state. Array and Reclaimer may loop because their operation is continuous.
Projectile count, launch point, timing, and damage reports must agree with the
animation.

## Approve and promote

1. Record the source/control SHA-256 and provenance, then present stable
   numbered options with the exact control included. Include animation only for
   assets whose role actually moves.
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
boundaries. Before running broad quantitative gates, record the human fidelity
verdict for the calibration set and stop if it has not passed. For a review-only
bank, also prove that a representative candidate is ignored with
`git check-ignore -v`, that the ledger has unique stable IDs, and that each
session's supported-media count equals its intended candidate count. Finish with
repository-wide tests, Clippy, and format checks from `AGENTS.md`.
