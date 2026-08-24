---
name: agent-notes
description:
  Upon user direction only, maintain durable Oxide agent workstream notes under
  agent-notes with Kladde. Use when planning, tracking, handing off, or closing
  multi-step repository work, or when recording agent-facing goals, decisions,
  findings, actions, and open questions.
---

# Oxide agent notes

Use Kladde to keep one durable note for each bounded workstream. The note is a
decision record and execution ledger that can survive handoffs and context
compaction, not a transcript of an agent session.

Agent notes live in the repository's `agent-notes/` notebook. You should only
create them when directed by a user. Do not create or update notes on your own
initiative. You may ask the user if they wish to record this activity in a note,
but do not create a note without explicit direction. Once the note has been
created, you may maintain it without explicit user direction.

## Choose the note boundary

Create or reuse one named note for a coherent initiative such as a repository
reset, bot rewrite, or rendering pass. Continue that note across sessions and
agents. Do not create one note per agent, day, or implementation attempt.

Use this stable shape:

```markdown
# Workstream name

## Goal

One sentence describing the successful outcome.

## Decisions

- Decisions made before the work is started or while the work is in progress;
  these should include what is and is not in scope, and any constraints on the
  work.

## Findings

- Non-obvious fact that changed or constrained the work.

## Actions

- [ ] Outcome-level task written before work begins.
  - Concrete action or evidence belongs here. This is where you record what you
    did, what changed, and what was verified. Keep the task open until the
    outcome is genuinely achieved.

## Open Questions

- A genuine unresolved decision or unknown, to surface to the user later.
```

Keep the goal to one stable sentence. Record choices and their reasons under
Decisions. Findings are facts a future agent would otherwise have to rediscover;
phrase them so they remain accurate after a later fix.

Seed Actions with outcome-level tasks before implementation. Keep a task open
until its outcome is genuinely achieved. Nest concise evidence beneath it: what
changed, what was verified, and any limitation that remains. Do not turn the
note into a file list, command transcript, or routine test-count report.

Open Questions contains only unresolved choices or unknowns. When one is
answered, remove it and preserve the answer under Decisions. A known actionable
gap belongs in Actions as an unchecked task, not in Open Questions.

## Make every semantic edit through Kladde

Use `--notebook agent-notes` and target the named note explicitly. Kladde's
locking and atomic replacement keep concurrent agents from losing one another's
updates. Do not use shell redirection or direct file writes for ordinary note
changes.

Create and seed a note through Kladde:

```sh
kladde new repo-reset.md --notebook agent-notes
kladde append "# Oxide Repository Reset" repo-reset.md --notebook agent-notes
kladde append "## Goal" repo-reset.md --notebook agent-notes
kladde append "Restore a clear development baseline." repo-reset.md \
  --notebook agent-notes
kladde append "## Decisions" repo-reset.md --notebook agent-notes
kladde append "## Findings" repo-reset.md --notebook agent-notes
kladde append "## Actions" repo-reset.md --notebook agent-notes
kladde append "## Open Questions" repo-reset.md --notebook agent-notes
```

Add a task, its evidence, and then check it only after completion:

```sh
kladde append "- [ ] Reconcile the architecture documents." repo-reset.md \
  --notebook agent-notes --under Actions
kladde append "- Corrected the documented tick pipeline." repo-reset.md \
  --notebook agent-notes --under Actions \
  --under-bullet "[ ] Reconcile the architecture"
kladde check --match "Reconcile the architecture" repo-reset.md \
  --notebook agent-notes --under Actions
```

`append` writes text verbatim, so include the leading `-` for a bullet. When
nesting under an unchecked task, `--under-bullet` includes its `[ ]` marker;
append evidence before checking the task. Let Kladde choose indentation.

Use placed appends, `check`, `uncheck`, and `remove` rather than guessing at
line positions. If a fact becomes stale, retract or rewrite it instead of
leaving the note internally contradictory. At a natural breakpoint and before
handoff, reconcile task states, findings, decisions, and open questions.

Prettier may normalize the Markdown after Kladde releases the notebook lock. Run
the repository Markdown formatting gate before considering the note update
complete.
