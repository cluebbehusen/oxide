# Performance and persistence regression procedure

Use this procedure for changes to simulation hot paths, bot preparation or
scheduling, retained caches, checkpoint fields/encoding, or shell session
transitions. A local helper or documentation edit does not require the complete
matrix. Select the affected rows and state the scope before measuring.

## Portable regression checks

These run in the existing workspace test jobs; no wall-clock CI threshold is
needed. Run the focused checks while developing:

```sh
cargo test -p oxide-bot --lib --locked opaque_payload_uses_a_compact_byte_string
cargo test -p oxide-bot --lib --locked completed_field_storage_scales_with_recipes_not_distance_arrays
cargo test -p oxide-bot --lib --locked recipes_preserve_mixed_progress_and_completed_overlay_distances
cargo test -p oxide-bot --lib --locked planning::tests::
cargo test -p oxide-kit --lib --locked bot_execution::background_tests::
cargo test -p oxide-shell --locked saved_game::tests::
```

The size guards cover distinct failure modes:

- A 4 KiB opaque controller payload permits at most 128 bytes of CBOR envelope
  overhead. Integer arrays or textual byte expansion must not silently replace a
  byte string.
- Thirty-two completed 128×128 approach fields must encode within 32 KiB,
  including their shared blocked grid. They are created through ordinary field
  requests, restore exactly, and answer with zero new work. Partial searches
  retain their progression and use the existing mixed-progress round trip.
- The existing 500-unit mass-battle generator, with both seats controlled, and
  the shipped seven-bot Skyhook scenario advance 24 ticks, then save through the
  production format. Each file must remain within 1 MiB compressed and 8 MiB
  decoded. Both resume for another 24 ticks with identical commands and worlds.
  These are short staged scale checks, not late-game performance measurements.
- The metadata test exercises the same reader used by file inspection and
  refuses any read or seek into the payload. A corrupt payload remains
  header-eligible but must fail full load.

These are generous ceilings, not exact serialization goldens. An intentional
increase needs a workload-specific explanation and measurements before changing
the ceiling. Do not relax deserialization safety bounds to satisfy a size test.
Existing shared-budget, production-reserve, pending-work, and background-seat
continuation tests remain required; do not replace them with whole-match hashes.

## Reproducible native workloads

Build once before timing, from the workspace root:

```sh
cargo build --release --locked -p oxide-driver -p oxide-shell
mkdir -p replays/performance
cargo run --release --locked -p oxide-driver -- run scenarios/skirmish.json --bots --ticks 720 --save-replay replays/performance/duel.json
cargo run --release --locked -p oxide-driver -- run scenarios/compass-grand.json --bots --ticks 6480 --save-replay replays/performance/grand.json
cargo run --release --locked -p oxide-driver -- run scenarios/skyhook-anchorage.json --bots --ticks 6480 --save-replay replays/performance/skyhook.json
```

Use the authored scenario seeds and configured bot profiles. `--bots` retains
one passive local seat and the configured opponents; `--all-bots` is a different
workload. Preserve these generated inputs for the candidate/control comparison.
Do not independently regenerate both sides and assume their worlds are equal.
Record the source commit and any setup/profile changes beside the results.

```sh
cargo run --release --locked -p oxide-driver -- profile-shell replays/performance/duel.json --from 240 --to 720 --speed 1
cargo run --release --locked -p oxide-driver -- profile-shell replays/performance/grand.json --from 6000 --to 6480 --speed 1
cargo run --release --locked -p oxide-driver -- profile-shell replays/performance/skyhook.json --from 6000 --to 6480 --speed 1
```

The profiler opens a 1280×800 window with disposable settings and data under
`target/oxide-profile`, removed when the shell exits. It requires a
scenario-origin replay, reconstructs its prefix, then runs current controllers
live. It does not accept `.oxsave` files or world-only replay origins. If the
match is already decided or leaves Playing during the window, choose and record
a comparable nonterminal interval; do not silently shorten the run. For
retention or late-game changes, additionally generate a longer record and
declare the matched late interval before comparing results. For scheduling
changes, repeat at 8× and 64× and report their throughput separately from
ordinary-speed frame headroom. Early dispatch has less time to help during
catch-up.

Run at least three isolated candidate/control trials without competing builds,
coverage or other benchmark jobs. Keep machine, resolution, DPI, camera,
profile, seed and interval fixed. Report absolute median and tail frame work,
frame intervals, over-budget counts, achieved tick rate, and unit/building
counts. Use 16.7 ms as the 60 FPS frame budget; do not gate shared runners on
that time. Investigate a repeatable regression above 10% with absolute costs and
variance. Controller throughput alone cannot establish a native frame
improvement.

## Save/load and catalog transitions

Use the native shell for these measurements. The live protocol remains usable
for UI/status queries while a persistence operation owns the session.

For an isolated automated session, launch from a separate terminal. This changes
only the child process environment and prints the disposable profile location:

```sh
uv run - <<'PYTHON'
import os, pathlib, subprocess, tempfile
root = pathlib.Path.cwd()
qa = pathlib.Path(tempfile.mkdtemp(prefix="oxide-performance-"))
env = dict(os.environ, HOME=str(qa / "home"), APPDATA=str(qa / "data"),
           XDG_DATA_HOME=str(qa / "data"), XDG_CONFIG_HOME=str(qa / "config"))
executable = root / "target" / "release" / ("Oxide.exe" if os.name == "nt" else "Oxide")
print(f"QA profile: {qa}", flush=True)
subprocess.run([str(executable), "--debug-server", "--profile-frames", "--paused",
                "--automation", "--window", "1280x800"], env=env, cwd=root, check=True)
PYTHON
```

In another terminal, use the same release driver:

```sh
driver() { cargo run --release --locked -q -p oxide-driver -- "$@"; }
driver live load-replay replays/performance/skyhook.json
driver live ui
```

1. Launch an isolated QA profile with
   `--debug-server --profile-frames --paused --window 1280x800`. For unattended
   input add `--automation`; then use
   `driver live load-replay <scenario-origin replay>` to establish the scene. Do
   not combine automation startup with `--replay` or use real player save
   directories for benchmark duplication/deletion. Keep the QA profile until its
   reproductions and measurements are no longer needed.
2. Through Pause → Save Game, produce an early and a representative late save.
   Record scenario, seed, profiles, tick, entity counts, file bytes, and the
   hardware/build identity. This uses the actual player-save encoder.
3. Bracket each UI operation with `driver live performance --reset`. Exercise
   named Save, shelf Load, Continue, and entering a shelf containing sixteen
   copies of the same checkpoint under distinct filenames in the QA profile.
   Wait for the catalog to populate before choosing conditional rows. Inspect
   `driver live ui` instead of assuming a fixed menu index.
4. Separately record elapsed operation time and frame work. Named saves stay
   paused; a load presents its loading screen before dispatch and opens paused.
   Capture screenshots outside the timing window. Querying or loading through a
   debug request outside frame timing does not measure a UI transition stall.
5. Repeat cold-process operations and at least five warm cycles separately.
   Include saving immediately after loading, rather than warming away recovery
   initialization or worker-admission delays. For worker/lifecycle changes,
   exercise cancel/retry and repeated replacements; check memory at comparable
   settled points and verify the previous session survives cancellation.

On the selected comparison machine, investigate warm 16-save discovery above 100
ms, capture/install p95 above 4 ms, any repeatable capture/install frame over
16.7 ms, and any repeatable save/load frame over 33.3 ms. A responsive loading
screen does not excuse a long total operation: report both. These are review
thresholds, not portable unit-test assertions. Cold spikes and unexplained
outliers remain in the report.

Keep generated replays and timing reports out of production commits. Retain
minimal reproducible inputs/generators and useful results; do not accumulate an
archive of every trial. The maintained scenarios, generators, tests and commands
above must remain sufficient to start again from a fresh checkout.
