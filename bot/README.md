# oxide-bot

`oxide-bot` owns the deterministic rules-based opponent. It depends on
`oxide-sim`; the simulation never depends on controller policy. Bots issue
ordinary commands and receive no special costs, information, or rules.

The [architecture map](../docs/bot-architecture.md) identifies owners,
transaction boundaries, execution, and focused domain references.

## Boundary and responsibilities

- `Brain` decides from an `Observation`. It cannot inspect authoritative state.
- `SeatBot` adapts a live state for the shell and runner: it gates finished
  matches, cadence and resignation, then captures a fog-honest player
  observation unconditionally.
- `oxide_sim::observation` owns fog filtering and the serialized knowledge
  schema. `Observation` adds lazy, bot-owned navigation inputs to that data.
- `PublicMapBriefing` shares immutable authored terrain across seats.
- `ResolvedProfile` resolves the scenario's difficulty, stance and personality
  seed. The scenario stores configuration without invoking bot policy. Derived
  `Dials` contain only live policy controls; their default uses the same
  Standard Balanced configuration as `BotConfig::default()`.
- Domains own evidence, proposals and tactics; allocation owns exact resources;
  the Executive owns command lowering. Retained and fresh work use one admission
  pipeline.

The host's `oxide_kit::bot_execution` owns worker scheduling. Each seat captures
its observation and runs its controller on the same worker. Immutable briefing
and navigation data can be shared; each seat keeps its mutable planning state.
Commands are gathered in canonical seat order before `State::tick`. This crate
adds no threads and no wall-clock decisions.

`SeatBot::checkpoint` captures controller memory and unfinished planning by
borrowing the seat. Restoration checks a separately versioned CBOR payload
against the scenario, configuration, and world boundary. Decision memory and
budgeted search progress survive; observational query caches and public-map
navigation caches and policy dials rebuild from the scenario configuration. The
payload is an internal same-version continuation format. It remains at revision
1 while the save system is unpublished; no legacy layout migration is provided.
It is separate from policy rollback checkpoints and never enters authoritative
`State` or its hash.

`navigation` owns bot route queries, search storage, and cache lifetimes. Its
`commands` module projects ordinary movement and Build routes from player
knowledge; `paths` provides canonical endpoint routes and bounds;
`public_fields` provides terrain and danger-aware travel distances; `service`
retains producer and target connectivity; `egress` certifies producer exits;
`flood` handles connectivity and placement witnesses; and `travel` converts a
route cost into free-flow ticks for a unit kind. Barricade foothold valuation
asks for costs instead of full paths. Planners supply knowledge, safety rules,
and target preferences; navigation preserves command orientation, path ties, and
search limits. Ground, air, and hypothetical layouts retain separate bounded
caches.

Voluntary defense rejects construction kinds that cannot meet the current
allocation reserve before searching for sites. Final allocation still owns the
exact funding and compatibility decision.

Internal operation planners and utility policy are not host entry points. Hosts
use `SeatBot`; observation-driven tooling can use `Brain`, and diagnostics
retain public trace and operation value types. Omniscient observation
construction is explicit QA infrastructure, never a configurable opponent
capability. Component tests supply admission inputs to the same planning
implementation as production.

## Development

Strategy's `think_alone` fixtures advance a single planner with fixture funding.
They select the richest active revision and then use the real resource
scheduler; they do not exercise portfolio competition or controller admission.
`Brain` tests own command-level funding, preemption and cross-domain ownership;
allocation-session tests own transactional rejection and rollback. Test-only
roster references and work counters verify query equivalence and bounds without
adding another runtime execution mode. Force-package fixtures supply the same
bounded `PlanningWork` as production; missing planning state defers purchases.
Tests comparing completed packages explicitly resume partial work within the
unchanged decision allowance. The exhaustive funding reference is only an
independent oracle, never a fallback inside the planner.

```sh
cargo test -p oxide-bot --locked
cargo test -p oxide-bot --lib utility::policy_tests --locked
cargo clippy -p oxide-bot --all-targets --locked -- -D warnings
```

Observation filtering tests live in `oxide-sim`; controller and bot-match tests
live here. Fixtures use observations, scenario construction, ordinary commands,
or validated state deserialization. The simulation exposes no test mutation API.

Bot observation is an optional callback surface in `observer`. Callbacks report
paired phase boundaries, deterministic planning-work counts, and opt-in
caller-attributed query work around the ordinary controller. They do not
introduce clocks, serialized fields, or alternative planning paths; callers own
timing and persistence.

See [Bot architecture](../docs/bot-architecture.md) for ownership contracts and
[Bot strategy](../docs/bot-strategy.md) for the intended opponent behavior.
