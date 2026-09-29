# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. The [specification](../docs/oxide-opponent.md) is normative for
this crate, and the
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds its
working rules.

Each decision spends through one running total in a fixed precedence:
emergencies, then workers, then production. A seat with no Harvester alive or
queued queues one even behind other work. Each built Foundry works the four
nearest live known scrap nodes its ground can reach within twelve tiles, none
shared with another Foundry, and idle Foundries train Harvesters until there are
two per worked node. Idle Harvesters go to the reachable worked node with the
fewest Harvesters. Idle Foundries then train Sentinels. Difficulty caps the unit
orders one decision issues; purchases do not count against that allowance. It
does not yet save for investments, build, attack, scout or expand.

## Boundary

The crate depends on `oxide-sim` and `chassis`, never on `oxide-bot`. It reads
only its seat's fog-honest `ObservationData` and its own order events, and emits
ordinary `PlayerCommand`s. Its seats also share one immutable `MapModel`, built
once per match from the scenario's public map: ground components and the
authored starts. It decides on its difficulty's interval and stays silent once
the match is decided, after its seat surrenders, or while it has no built
Foundry. Equal-distance choices are broken in a frame anchored on the seat's
authored start, so mirrored seats make mirrored choices.

## Own events

Each decision also receives the seat's own `OrderStalled` and `CommandRejected`
events since its previous decision, oldest first. `OwnEvents` holds them: the
host keeps one per seat beside the controller and calls `record` after every
tick, and a decision takes them all. A buffer holds at most 64 events and drops
the oldest first. A tick without a decision leaves the buffer untouched. The
stub only reports the events in its trace.

## Selection

A seat runs this controller when its scenario `bot_config` names
`"controller": "opponent"`. `oxide-kit` hosts it next to `oxide-bot`, so live,
headless, saved and recovered sessions build it from the same scenario data.

## Checkpoint and trace

`Checkpoint` holds only the seat; the host saves the seat's `OwnEvents` beside
it. Restoring it checks that the seat is a configured `oxide-opponent` bot in
the bound scenario and world, rebuilds the profile and decision interval from
the scenario, and takes the map model built from it. A saved buffer over the cap
does not load.

`Opponent::act_traced` returns a `Trace` of the decision's tick, seat, bank,
received own events, committed spending, purchases, unit-order count and
allowance. Traces are diagnostics only.
