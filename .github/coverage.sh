#!/usr/bin/env bash
# Run both coverage gates from one instrumented build and publish a compact
# per-crate table for each to the job summary. The unit gate counts library
# and binary unit tests; the combined gate adds the integration suites to the
# same profiles. Kladde can annotate every miss because it enforces 100%;
# Oxide deliberately starts lower, where thousands of line annotations would
# bury useful output.
#
# The combined run skips the representative-map integrity soak:
# instrumentation makes it expensive while it adds little line coverage, and
# `cargo test` still runs it.
set -uo pipefail

gate() {
    local title="$1" floor="$2" status table report
    cargo llvm-cov report --summary-only --fail-under-lines "$floor"
    status=$?
    report="$(mktemp)"
    if cargo llvm-cov report --json --summary-only --output-path "$report"; then
        table="$(jq -r --arg root "$PWD/" '
            def identity:
                .filename
                | ltrimstr($root)
                | split("/")[0]
                | if . == "sim" then {rank: 1, name: "oxide-sim"}
                  elif . == "opponent" then {rank: 3, name: "oxide-opponent"}
                  elif . == "protocol" then {rank: 4, name: "oxide-protocol"}
                  elif . == "kit" then {rank: 5, name: "oxide-kit"}
                  elif . == "net" then {rank: 6, name: "oxide-net"}
                  elif . == "shell" then {rank: 7, name: "oxide-shell"}
                  elif . == "driver" then {rank: 8, name: "oxide-driver"}
                  else {rank: 0, name: .}
                  end;
            def row($name; $covered; $count):
                "| \($name) | \($covered) / \($count) | \((10000 * $covered / $count | round) / 100)% |";
            .data[0] as $data
            | ($data.files
               | map(identity as $id | {
                     rank: $id.rank,
                     name: $id.name,
                     covered: .summary.lines.covered,
                     count: .summary.lines.count
                 })
               | sort_by(.rank)
               | group_by(.name)
               | map({
                     rank: .[0].rank,
                     name: .[0].name,
                     covered: (map(.covered) | add),
                     count: (map(.count) | add)
                 })
               | sort_by(.rank)) as $crates
            | "| Crate | Covered lines | Coverage |",
              "| --- | ---: | ---: |",
              ($crates[] | row(.name; .covered; .count)),
              row("Workspace"; $data.totals.lines.covered; $data.totals.lines.count)
        ' "$report")"
        printf '%s\n' "$table"
        {
            echo "### $title (${RUNNER_OS:-local})"
            echo
            printf '%s\n' "$table"
        } >>"${GITHUB_STEP_SUMMARY:-/dev/null}"
    fi
    rm -f "$report"
    return "$status"
}

status=0
cargo llvm-cov clean --workspace
cargo llvm-cov --no-report --workspace --lib --bins --locked || status=1
gate "Unit coverage" 82.0 || status=1
cargo llvm-cov --no-report --workspace --test '*' --locked -- \
    --test-threads=1 \
    --skip representative_scenarios_preserve_state_integrity ||
    status=1
gate "Combined coverage" 84.0 || status=1
exit "$status"
