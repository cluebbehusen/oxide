---
created: 2026-09-06T07:27:17
updated: 2026-09-08T06:10:08
---

# Oxide presentation polish

## Goal

Make Oxide feel like a cohesive, readable, physically convincing RTS through an
integrated pass over its interface, art, motion, effects, and sound, evaluated
in ordinary native play.

## Decisions

- Preserve Oxide’s crisp pixel character, wave-pattern quarry floor and muted
  industrial palette. The smooth enclosed-machine redesign lost too much
  identity; retain the improved motion and readability without that visual
  language.
- Use one recognizable working mechanism per machine, supported by purposeful
  structural detail. Simplifying static machinery matters more than merely
  slowing a busy animation. Extractor is the economy-family material reference;
  Refinery needs visibly greater mass than Reclaimer.
- Keep pale outlines off weapons. Use restrained authored metal highlights on
  barrels, rails and mounts; chassis and foundations may retain an edge for
  ground contrast. Sentinel, Warden, and Breaker should have a clear size
  hierarchy, with Breaker only slightly larger than Avalanche.
- Animate the actual mechanism and state: recoil, loading, transfer, welding and
  scanning. Routine work should be quiet. Array and scout radars can scan
  continuously; production and repair animation must correspond to work.
- Avalanche launches a small missile with a short rail ejection followed by
  ignition and acceleration. Condor releases through opening nose panels.
  Bombard fires an unpowered shell that follows its barrel, with shadow and
  scale conveying height.
- Use Chakra Petch Medium for all interface text. Keep the build palette
  accessible while placing a building, show useful economic and operational
  information, and retain readable text while reducing wasted HUD space.
- Ground units rotate in place at rates informed by movement speed. Heavy guns
  and Buzzard’s independent turret must align before firing. Bombard aligns
  before bracing and unbraces before repositioning. These intentional gameplay
  changes trade instantaneous response for physical coherence.
- Preserve Condor and Moth’s distinctive flight and attack-run rules. Draw scale
  and collision size are separate contracts; an art resize does not authorize a
  balance change.
- Preserve floor tile delineation for placement, with narrow edge feathering.
  Vary stains and grates spatially. Pits and map borders are the same quarry
  material and must share strata and depth treatment. Low wreckage is passable;
  rocks and abandoned equipment read as ground blockers.
- Keep production sound effects unchanged in this PR. Experimental audio carried
  into visual reviews was not approved for release; handle replacement sounds as
  a separate listening decision.
- Regenerate affected replay fixtures at the existing version as explicitly
  approved. Do not bump the workspace or simulation version. Turning and
  serialized projectile changes are not replay-neutral.
- Retain local review material until the PR is merged. Production generators and
  documentation must stand alone without local candidate numbers or
  review-directory dependencies.

## Findings

- Shell-launched weapons route through shell_fire_sound. Named Avalanche and
  bomb-release clips do not by themselves prove those clips play for the
  corresponding launch event; verify routing during the sound follow-up.

## Open Questions
