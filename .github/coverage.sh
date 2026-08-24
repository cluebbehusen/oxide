#!/usr/bin/env bash
# Run a coverage gate and publish a compact per-crate table to the job summary.
# Kladde can annotate every miss because it enforces 100%; Oxide deliberately
# starts lower, where thousands of line annotations would bury useful output.
set -uo pipefail

gate="$1"  # cov-unit | cov-combined
title="$2"

cargo "$gate"
status=$?

report="$(mktemp)"
if cargo llvm-cov report --json --summary-only --output-path "$report"; then
    table="$(jq -r --arg root "$PWD/" '
        def identity:
            .filename
            | ltrimstr($root)
            | split("/")[0]
            | if . == "sim" then {rank: 1, name: "oxide-sim"}
              elif . == "protocol" then {rank: 2, name: "oxide-protocol"}
              elif . == "kit" then {rank: 3, name: "oxide-kit"}
              elif . == "shell" then {rank: 4, name: "oxide-shell"}
              elif . == "driver" then {rank: 5, name: "oxide-driver"}
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
exit "$status"
