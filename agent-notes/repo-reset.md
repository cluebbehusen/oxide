---
created: 2026-08-23T08:49:35
updated: 2026-08-24T09:20:33
---

# Oxide Repository Reset

## Goal

Restore a clear, trustworthy development baseline before introducing a
player-facing scripted opponent.

## Decisions

- Preserve the deterministic simulation, replay, fog-honest command boundary,
  and live QA architecture.
- Fix verified player-facing bugs before structural refactors, then refactor
  only surviving code along real ownership seams.
- Keep the root README concise and give each top-level Rust crate its own
  human-readable documentation.
- Treat art and sound finalization as separate, non-blocking work.
- Remove the complete ML training and runtime controller stack; versioned
  replays preserve emitted commands without retaining obsolete controller code.
- Keep The Scattering's island identity and make its air premise explicit with
  one mirrored faction-scout pair rather than weakening the shipped liveness
  gate.
- Keep skills self-describing: `AGENTS.md` owns global contracts, focused skills
  own repeatable procedures, and named Kladde notes own user-requested
  workstream history.
- Make building upgrades paid, self-timed work: the structure goes offline
  during the timer, progresses without a unit order, and returns at the upgraded
  tier.
- Treat an unfinished site as attended while a live own worker references it in
  an active or queued `Build` program; decay begins only after every such claim
  disappears.
- Keep one all-map liveness sweep, enrich it with reachable-state invariant and
  serialization checks, and exclude that expensive sweep from routine coverage
  instrumentation while retaining it in normal tests.
- Treat coverage as a discovery tool rather than a target: prioritize externally
  observable contracts, deterministic edge cases, and error paths; do not add
  tests that merely execute lines or mirror implementation details.

## Findings

- The initial worktree is mixed: main is current, with existing modified and
  untracked work that must be preserved during cleanup.
- Baseline Rust gates: formatting, workspace tests, and rustdoc passed; strict
  Clippy found one obsolete chunking idiom in the screenshot comparison helper,
  which was corrected without suppressing the lint.
- Crucible construction is mechanically valid but hidden on an unlabeled second
  build page; the displayed shortcut omits the extra page-toggle key.
- Extractor restoration accepts only the authored frame anchor, so three
  quarters of the visible 2x2 frame reject a natural click. The same path also
  reveals unseen frame claims through refusal and rendering behavior.
- The progression fixes preserve sim legality while making the build surface
  honest: the advanced palette is explicit, Extractor frames behave as one
  target, and hidden claims no longer leak through placement feedback.
- The Python training project, quantized runtime, profile machinery, and
  training-only driver surfaces were isolated and removed; incompatible
  historical replays remain protected by simulation versioning.
- A wizard click could launch a new seat without updating the shared cursor, so
  a persisted edge-pan setting dragged the fresh camera toward the top-left
  corner. Treating every pointer event as authoritative fixed the seat-swap
  regression.
- The Balanced AI completes duel, team, pit, and Extractor scenarios coherently.
  Large FFA and grand maps still expose repeated unreachable-route and
  invalid-build-site choices.
- Connor intentionally pruned some historical documents and untracked research
  notes during the reset; do not restore those deletions merely because they
  were present at baseline.
- The Scattering's starting scouts are a valid authored air-map choice, but they
  expose a generic controller bootstrap gap: Balanced never trains a scout
  flyer, retains a rejected ground scout forever, and cannot discover the target
  that unlocks its existing air and ferry logic.
- Architecture review found the simulation document's tick pipeline and economy
  coverage incomplete, and its source map missing the new placement module.
- Architecture review found the shell document overclaimed screenshot and
  `RawEvent` boundaries and omitted socket-thread and dual-playback-world
  constraints.
- `State::validate_invariants` accepts unfinished construction progress beyond
  the active build timer; the next automatic or worker-driven construction tick
  can underflow in debug and wrap in release.
- Current macOS coverage is 67.17% from 620 unit tests and 75.24% from the full
  1,111-test combined surface; simulation reaches 96.42% combined, while the
  Macroquad shell remains 62.45%.
- The two all-map simulation sweeps add only 0.30 points of workspace coverage
  but increase the instrumented run from about two minutes to about 26 minutes;
  ordinary CI already runs both at full length.
- The remaining highest-value structural seams are the shell frame loop and the
  scripted bot utility and executive modules; stats and several other large
  files are mostly cohesive data or focused behavior.
- All four reviewed regressions were valid: legacy bot setup failed before
  version validation, malformed upgrade progress could underflow, advanced-page
  shortcuts lied about their input path, and the generated map review was
  unignored.
- After the new regressions and structural splits, macOS coverage is 67.23% unit
  and 75.06% fast combined; the four core libraries remain above 95% combined.
- Repeated normal and instrumented Rust builds had grown `target/` to 17 GiB;
  `cargo clean` removed 20.1 GiB of Cargo artifacts before final validation.
- Final cleanup removed another 4.6 GiB after validation, for 24.7 GiB of Cargo
  artifacts removed across the pass.
- The later coverage and native QA cycle regenerated another 7.7 GiB of Cargo
  artifacts; the final clean removed them, bringing the reset's cumulative
  artifact cleanup to 32.4 GiB.

## Actions

- [x] Remove training-only Python and its integrations.
  - Deleted the tracked Python training project, checkpoints, and its local
    environment and caches; the tracked source remains recoverable from Git
    history.
  - Removed the training CI job and Rust/Python gym handshake while retaining
    deterministic sprite, sound, and map validation.
- [x] Remove proven orphaned code and dependencies, then refactor surviving Rust
      where warranted.
  - Removed neural, gym, profile, ladder-weight, composition, training-probe,
    and dead protocol surfaces together with obsolete dependencies and tests.
  - Split placement ownership from the oversized `State` module into
    `sim/src/state/placement.rs` and narrowed surviving helpers to their real
    callers.
  - Corrected the stale vision contract comment that still described the removed
    built-in opponent as an omniscient cheating bot.
- [x] Rewrite repository documentation and agent workflows against the cleaned
      tree.
  - Rewrote the root README and added concise README-backed crate documentation
    for chassis, simulation, protocol, kit, shell, and driver.
  - Condensed `AGENTS.md` into a global contract and router; replaced the
    bot-training workflow with focused simulation-development and scripted-bot
    skills and refreshed map-authoring guidance.
- [x] Complete broad QA and record remaining known gaps.
  - Full formatting, strict Clippy, workspace tests, and rustdoc gates passed at
    `0.15.9`; existing state-hash rows stayed unchanged.
  - Native smoke passed 14/14; real-shell checks covered progression, all four
    Extractor-frame tiles, replay parity, and alternate-seat camera focus.
  - Fixed alternate-seat camera drift by updating the shared cursor from mouse
    move, press, and release events before screen dispatch.
- [x] Build and mechanically validate one initial scripted opponent.
  - Replaced every shipped opponent configuration with one strict `scripted`
    controller; the setup wizard now presents only Balanced AI and the driver
    exposes `--all-bots` for complete-match evaluation.
  - Kept `Brain::overseer` as a separate QA anchor while `Brain::balanced` uses
    the fog-honest utility and executive command path available to players.
  - Skirmish Basin, Twin Forges, Severance, and Powder Keg completed coherently;
    Compass Grand and Skyhook Anchorage exposed invalid-site and
    unreachable-route loops that remain open.
- [ ] Have Connor play against and watch the Balanced AI before tuning or
      treating it as ready.
- [x] Establish baseline gates and reproduce the Crucible and Extractor-frame
      reports.
  - Exposed explicit Basic and Advanced build pages; Crucible is visible and the
    keyboard path is `B`, `B`, `3`.
  - Canonicalized every visible tile of a 2x2 Extractor frame to its anchor
    across simulation legality, placement previews, ghosts, range checks, pings,
    and deferred commands without leaking unseen claims.
- [x] Promote The Scattering into the shipped scenario roster.
  - Moved `the-scattering.json` into `scenarios/`, migrated it to the scripted
    controller, removed its stale neural-era duration claim, and removed the
    empty `map-drafts/` directory.
  - The original ground-only roster deadlocked at 2% exploration with 9,845
    commands and no shots by tick 40,000; mirrored Kestrel and Gnat scouts made
    the air-first premise explicit.
  - With scouts, first contact landed at 0:45, both sides produced Skyhooks,
    five battles occurred, and Cupric won at 9:06 with 342 commands and no
    command rejections.
  - The map audit, symmetry and fairness gates, shipped liveness sweep, full
    workspace gates, and new reviewed hash row all pass.
- [x] Reconcile the architecture documents with the cleaned implementation.
  - Corrected the simulation document to match the exact tick pipeline, current
    economy and placement rules, and maintained source and test entry points.
  - Corrected the shell document to distinguish UI input from semantic debug
    commands, GPU from CPU screenshots, and visible playback from the hidden
    live session.
  - Re-ran formatting, strict Clippy, workspace tests, and rustdoc successfully.
- [x] Establish agent-note and Markdown quality gates.
  - Added a Kladde-backed agent-notes skill, its UI metadata, and the shared
    Claude alias.
  - Adopted Kladde's pinned Prettier configuration and formatted every
    nonignored Markdown file, including hidden skills and agent notes.
  - Added dedicated Markdown and all-skill validation jobs to CI and documented
    both local gates in `AGENTS.md`.
  - Forward-tested the note workflow and passed both the Codex and reference
    skill validators.
  - Full Rust gates, repository-wide Prettier, and all seven canonical skill
    validations passed.
- [x] Clarify the boundary between AGENTS.md, skills, and agent notes.
  - Removed the hand-maintained skill router from `AGENTS.md`; kept
    repository-wide contracts there and directed repeatable procedures to
    self-describing skills and user-requested workstream history to Kladde
    notes.
- [x] Reproduce and correct the replayed Avalanche and Condor movement defects.
  - Replayed the full 19-minute Deep Cut autosave and traced the reported units
    and commands tick by tick.
  - Made Avalanches retreat to the nearest reachable legal firing stand when a
    target is inside their four-tile blind ring.
  - Coordinated head-on collision slides and let nearby bodies advance an
    intermediate waypoint after crossing its onward plane, eliminating the
    replayed two-frame oscillation without weakening terrain or displacement
    caps.
  - Routed wide-turn aircraft from their exact position, preserved Peak detours,
    invalidated blocked flight paths, and kept bomber egress segments Peak-free;
    the Condor regression now completes repeated passes without extra turn rate
    or long stalls.
  - Added replay-shaped combat, collision, and Peak regressions, moved the
    deterministic simulation to `0.15.10`, and regenerated the state-hash
    fixture.
  - Passed all workspace Rust gates, the repo-wide Markdown check, and all seven
    canonical skill validators after the fixes.
  - Moved the showcase Turret clear of a changed shell arc and reviewed both
    changed renderer goldens.
- [x] Explain the replayed upgrade-worker and construction-site behaviors.
  - Confirmed that an upgrade click drafts the three nearest harvest-capable
    units regardless of their jobs and sends a replacing order; one replayed
    upgrade stole a worker from the active Crucible build.
  - Confirmed that paid queued sites decay while their assigned worker is
    traveling or completing earlier queue entries; the fixed construction ramp
    preserves that decay as missing completion hp.
  - Identified the narrow cleanup rule: a site referenced by a live worker in an
    active or queued `Build` program can remain protected, while a genuinely
    unclaimed site still decays.
- [x] Replace worker-driven upgrades with deterministic building progress and
      make site decay claim-aware.
  - Changed `UpgradeBuilding` to a building-only command and moved every upgrade
    onto a structure-owned deterministic timer; it remains offline, vulnerable,
    non-cancellable, and unable to use workers for speed.
  - Reused `Building.progress` and the shared damage-first work resolver,
    preserving exact replay determinism, one completion event, vulnerability
    during downtime, and lethal-fire-wins ties.
  - Restricted ordinary `Build` and deferred-founding paths to tier-zero sites;
    stale or explicit worker orders cannot join or accelerate an upgrade.
  - Made abandonment decay claim-aware for tier-zero sites: active and queued
    live worker commitments protect paid work, while genuinely unclaimed sites
    still decay; self-upgrades never enter the decay pass.
  - Updated Balanced AI lowering and orphan recovery, the protocol shape, shell
    cards and progress feedback, right-click behavior, cancellation handling,
    and construction animation to match the new contracts.
  - Added regressions for exact zero-worker timing, untouched worker programs,
    stale and explicit build orders, decay protection and release, mortality,
    completion-tick fire, bot behavior, protocol coverage, and shell input and
    presentation.
  - Bumped the deterministic simulation to `0.15.11`, regenerated and visually
    reviewed the hash and renderer fixtures, and replayed a live zero-worker
    upgrade to the same final hash before passing every repository gate.
- [x] Harden deserialized construction state.
  - Reproduced an accepted tier-one Turret with progress past its timer
    panicking on the next tick; validation needs mode-aware progress bounds and
    adversarial coverage.
  - Rejected unfinished tiered buildings whose progress exceeds that tier's
    construction timer, and added a forged tier-one Turret regression that fails
    during deserialization instead of underflowing on the next tick.
- [x] Add unit-only and combined Rust coverage gates.
  - Measured per-crate and workspace baselines with `cargo llvm-cov`; no
    coverage aliases, thresholds, or CI job exist yet.
  - Added `cargo cov-unit` and `cargo cov-combined` aliases plus a dedicated
    pinned Linux CI job; current macOS baselines are 67.23% unit and 75.06% fast
    combined against 67.0% and 74.5% floors.
- [x] Finish the remaining targeted Rust cleanup.
  - Normal formatting, Clippy, tests, rustdoc, dependency policy, and diff
    checks pass; remaining work is focused rather than a broad rewrite.
  - Split the shell frame loop and bot policy seams deliberately, remove
    confirmed orphan assets and APIs, tighten overly broad visibility and
    mutable access, and rewrite residual experiment-history comments as timeless
    constraints.
  - Split the shell screen flow and scripted-bot utility and executive modules
    along internal ownership seams while preserving the 4,000-tick bot hash
    exactly.
  - Restricted production simulation and pending-command mutation to `Game`,
    removed confirmed orphan assets and helpers, replaced stale historical
    comments with current constraints, and passed an added redundant-clone
    Clippy sweep.
- [x] Harden agent-facing protocol and replay inputs.
  - Live probing found that unknown debug-protocol fields are silently accepted,
    and replay JSON is parsed without a byte or command-count ceiling before
    validation.
  - Made request envelopes and request parameters reject unknown fields, capped
    replay input at 64 MiB and one million commands, and covered oversized
    files, misspelled requests, and valid neighboring shelf records.
  - Normalized only the known historical bot-config schema before version
    validation, allowing legacy replay archaeology without weakening current
    scenario parsing.
- [x] Consolidate the duplicate all-map simulation sweeps.
  - Kept one exhaustive shipped-scenario liveness sweep, added invariant checks
    every 100 ticks and JSON round trips every 500 ticks, and removed the
    duplicate simulation sweep.
  - Retained the full sweep in ordinary tests but skipped only that test under
    LLVM instrumentation; it passes normally in about 21 seconds.
- [x] Raise coverage through behavior-driven tests across high-value uncovered
      seams.
  - Audited fresh combined line and function coverage before writing tests; the
    largest meaningful gaps were shell screen ownership, driver input and replay
    orchestration, scripted-bot lowering and strategic fallbacks, and hostile
    persistence and framing boundaries.
  - Added behavior-level regressions for bot recovery, threat domains, stalled
    armies, construction and ferry reserves, observation privacy, transport
    races, replay and session replacement, bounded socket frames, shell clocks
    and seeks, menu semantics, persistence failures, and result interactions.
  - Rejected renderer, native-window, audio-device, long timeout, private
    search-order, and impossible valid-state branches where a unit test would
    only execute lines or freeze implementation details.
  - Added reproducibility and format coverage for the application icon plus
    end-to-end map-review assembly, and wired the maintained review-tool tests
    into CI.
  - Strengthened the audit-discovered weak spots: map and team fixtures can no
    longer silently skip, touch tests prove finger ownership and cancellation,
    and test-only shell routes plus orphan bot intents were removed instead of
    preserved for coverage.
  - Closed a real live-driver hole by routing raw commands through the strict
    debug-wire boundary before socket access; added compiler-exhaustive key
    coverage and exact profile-window validation.
  - Raised measured macOS coverage from 67.23% to 70.52% unit and from 75.06% to
    77.73% fast combined; raised enforced floors from 67.0% to 69.0% and from
    74.5% to 76.5% with platform headroom.
  - Passed formatting, strict Clippy, the complete workspace suite including the
    all-map soak, rustdoc, both raised coverage gates, asset and review-tool
    checks, skill validation, and the 14-check native smoke harness.
  - The final cargo clean removed 7.7 GiB after the heavy coverage and native QA
    cycle.
- [ ] Bound matchup roster input and use checked cost arithmetic.
  - Army and garrison parsers accept arbitrary 32-bit counts; extreme local CLI
    input can overflow cost arithmetic or request excessive allocation. Choose
    and document an explicit count or total-budget policy before hardening it.
- [ ] Extract a renderer-independent shell transition and request seam.
  - Much of screen flow remains interleaved with Macroquad drawing, autosaves,
    process exit, and live-session ownership. A focused reducer seam would
    permit meaningful transition and request tests without mocking the renderer.

## Open Questions

- Does the Balanced AI feel coherent and fun in human play?
- How should large-map unreachable-route and invalid-build-site loops be
  addressed before broad-map bot tuning?
- Should Balanced maintain one faction scout after Airworks and replace a
  stranded ground scout, so future island maps do not require a special starting
  roster?
