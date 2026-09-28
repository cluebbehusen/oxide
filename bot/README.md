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

[Controller checkpoints](../docs/bot/controller.md#controller-checkpoints)
preserve decision memory and deterministic planning progress; observational data
is rebuilt. [Allocation](../docs/bot/allocation.md) defines exact commit and
rejection, and [combat](../docs/bot/combat.md) defines mission and experience
ownership.

`navigation` owns bot route queries, search storage, and cache lifetimes. Its
`commands` module projects ordinary movement and Build routes from player
knowledge; `paths` provides canonical endpoint routes and bounds;
`public_fields` provides terrain and danger-aware travel distances; `service`
retains producer and target connectivity; `egress` certifies producer exits;
`flood` handles connectivity and placement witnesses; and `travel` converts a
route cost into free-flow ticks for a unit kind.
[Navigation](../docs/bot/navigation.md) describes their cache and work
contracts.

Internal operation planners and utility policy are not host entry points. Hosts
use `SeatBot` through `oxide-kit`'s `SeatController`; observation-driven tooling
can use `Brain`, and diagnostics retain public trace and operation value types.
Omniscient observation construction is explicit QA infrastructure, never a
configurable opponent capability. Component tests supply admission inputs to the
same planning implementation as production.

## Development

Strategy tests run the air planner through the shared `strategy::fixtures::turn`
fixture, which executes one decision in production order against an allocation
of the planner's own claims, without competing domains. `Brain` tests own
command-level funding, preemption and cross-domain ownership; allocation-session
tests own exact admission and rejection.

Team relief keeps observed pressure age separate from its proposed force.
Proposals hold no units and are not persisted. A retry revalidates current
members without restarting pressure credibility. An admitted preparation holds
its exact group and home screen until pressure becomes credible; then it
dispatches the group and releases the screen.

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
