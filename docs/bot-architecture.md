# Bot architecture

Oxide has one fog-honest controller. [Bot strategy](bot-strategy.md) defines its
intended behavior; this page maps the implementation's owners and boundaries.
Detailed domain contracts live in the focused references below. Rust types own
schema details, the
[scripted-bot skill](../.agents/skills/scripted-bot/SKILL.md) owns repeatable
procedures, and agent notes retain historical investigations.

## Ownership map

| Owner                      | Responsibility                                                                   | Boundary                                                                             |
| -------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `oxide-sim`                | Rules, command validation, authoritative state, player-knowledge projection      | Knows nothing about bot policy. `State::tick` alone changes the game.                |
| `SeatBot`                  | Finished-match, cadence and resignation gates; fog-honest observation capture    | Host adapter on the seat's worker. `Brain` receives observations.                    |
| `Brain`                    | Coordinate observation, memory, maintenance, allocation, utility and lowering    | Owns one seat's decision state; no direct simulation mutation.                       |
| Domain planners            | Propose investments; retain objectives, cohorts and operation lifecycle          | Describe exact needs using player knowledge; do not own a second resource scheduler. |
| `AllocationSession`        | Reconcile obligations, compare proposals, fund exact claims, validate and commit | Canonical authority for shared capital, actors, sites and producer schedules.        |
| `UtilityPolicy`            | Economic/support assessment and residual tactical planning                       | Uses the resources and exclusions granted by admission.                              |
| `Executive`                | Army lifecycle, mission ownership, intent-to-command lowering                    | Preserves ordered actor claims and produces ordinary commands.                       |
| `resources`                | Paid inventory, queue projections, capacity and egress                           | Predicts ordinary rules; allocation owns funded scheduling.                          |
| `navigation` / `planning`  | Prepared geometry, bounded queries and retained refinement                       | Cache lifetime and work allowance follow explicit knowledge and decision boundaries. |
| `oxide-kit::bot_execution` | Parallel seat execution and canonical command collection                         | No worker completion order enters a decision or simulation result.                   |

## Decision and transaction boundaries

1. The seat captures a player observation. Intelligence and battlefield memory
   interpret only that evidence and immutable authored terrain.
2. Retained work observes progress before fresh work competes. Observation facts
   and learning survive rejection of speculative ownership changes.
3. Allocation imports existing claims and selects compatible exact proposals.
   Paid queue entries, unpaid future jobs, and unspent reserved capital retain
   distinct meanings. One FIFO projection governs producer timing.
4. Commit validates every selected owner update before applying any. Rejection
   discards new claims while retaining reconciliation and observed outcomes.
   Accepted air and lift orders run after adjudication, using committed members
   and current support signals.
5. Residual utility receives explicit resource limits and exclusions. Ground
   mission planning services retained assignments, then defense, then pressure
   and reserves. These stages share one decision-local claim and attention
   state.
6. The Executive lowers intents in order. The host records commands and passes
   them to the simulation; bot memory and diagnostics never modify game rules.

Air operation admission and recovery are encoded in one lifecycle state.
Watching a remembered target differs from an admitted assault even though both
appear as Recon in diagnostics. Recovery carries its reason and prior admission.
Resource ownership still crosses several domains: a smaller file or a renamed
adapter alone does not simplify that transaction.

## Execution and performance

Independent seats run on the existing host pool of up to four workers. Each
worker captures its own observation and runs its controller; results join in
canonical seat order. Busy pools and nested match batches have serial paths.
Within a seat, allocation and tactical assignment remain ordered because later
choices consume earlier claims, attention, and producer capacity.

Immutable public-map topology can be shared. Observation navigation inputs,
operation preparation and army facts have narrower lifetimes; they cannot be
retained across changed knowledge without an invalidation contract. Bounded
planning fixes allowances before work and reports deferred progress explicitly.

Work counters, allocations, controller latency, native frame cost and match
throughput are different evidence. Fewer queries or allocations alone do not
establish a faster game. There is no additional per-domain worker pool.

## Focused implementation references

| Area                                                                       | Reference                       |
| -------------------------------------------------------------------------- | ------------------------------- |
| Profiles, fair information, host boundary, replay configuration and traces | [Controller](bot/controller.md) |
| Resource claims, admission ordering, producer funding and exact commit     | [Allocation](bot/allocation.md) |
| Economic investments, contested harvest and expansion                      | [Economy](bot/economy.md)       |
| Support, reconnaissance, connected offense, air/lift and defense           | [Operations](bot/operations.md) |
| Geometry preparation, route services, cache lifetimes and bounded work     | [Navigation](bot/navigation.md) |
| Army missions, contact, recovery and learning                              | [Combat](bot/combat.md)         |

## Test boundaries

Component fixtures establish ranking, geometry and operation transitions under
explicit supplied conditions. They do not establish controller admission merely
because they call the real scheduler. `Brain` tests exercise competing funding,
preemption, ownership and actual command output; allocation-session tests
exercise exact commit and independent maintenance under rejection. Independent
small oracles and fault injection remain useful when they probe those contracts.

Long-horizon command/state traces provide broad regression evidence. Human play
and native replay review remain the gate for credible, enjoyable behavior.
