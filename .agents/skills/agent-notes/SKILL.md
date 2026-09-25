---
name: agent-notes
description:
  Maintain a concise Oxide workstream decision brief under agent-notes with
  Kladde. Create one only at the user's direction; maintain an existing note
  when planning, handing off, or closing its authorized workstream.
---

# Oxide agent notes

Keep one living decision brief for a bounded workstream. It should let another
agent continue correctly without reading the conversation. It is not an activity
log. Create a note only when the user directs it; reuse and maintain it
thereafter within the authorized scope. A read-only request does not authorize
note edits.

## Preserve decisions, not a transcript

Use these headings when they have content:

- **Goal:** one stable sentence describing the intended outcome.
- **Decisions:** consequential choices, constraints and their reasons.
- **Findings:** established facts a successor needs, with useful evidence links.
- **Actions:** outcome-level tasks and their completion state.
- **Open Questions:** unresolved decisions or unknowns, not known work items.

Keep an agreed plan stable. Change it for an accepted scope decision or to mark
outcomes complete, not to append every implementation step. Update or replace a
stale finding; remove a resolved question and retain its decision only if
useful. Record partial or blocked outcomes honestly.

Omit routine test counts, file lists, PR-by-PR status, command transcripts and
unrelated discoveries. Put implementation history in version control and
detailed measurements in reproducible evidence. Include a link only when a
successor needs it; a committed handoff must not depend on private machine
paths. Summarize negative experiments by their useful conclusion and conditions,
not every trial.

At handoff or closeout, condense the note to the decisions, results, limitations
and remaining work that still matter. Prefer a short brief over another appended
section. Preserve useful material before deleting superseded notes or evidence,
and follow the user's cleanup scope; maintaining a note does not authorize
archiving or deleting unrelated work.

## Edit through Kladde

Target the named note explicitly with `--notebook agent-notes`. Kladde's lock
and atomic replacement prevent concurrent agents from losing changes. Use its
semantic operations rather than direct writes or guessed line numbers:

```sh
kladde read repo-reset.md --notebook agent-notes
kladde append '- [ ] Reconcile architecture guidance.' repo-reset.md --notebook agent-notes --under Actions
kladde check --match 'Reconcile architecture guidance' repo-reset.md --notebook agent-notes --under Actions
kladde remove --match 'Obsolete finding' repo-reset.md --notebook agent-notes --under Findings
```

Use `kladde new` only for a user-requested new note. `--under` selects headings;
`--under-bullet` selects a bullet by its text prefix (including `[ ]` for an
unchecked task). Include the leading `-` in appended bullets and let Kladde
indent children. Consult `kladde <command> --help` for other operations.

Format Markdown with Prettier after Kladde releases its lock. Keep the user's
daily note separate and follow its own capture rules.
