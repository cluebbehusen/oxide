# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. The [specification](../docs/oxide-opponent.md) is normative for
this crate, and the
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds its
working rules.

This crate is currently a stub. On each decision it sends idle Harvesters to the
nearest known scrap node and has each idle Foundry train a Harvester, up to six
per Foundry, or otherwise a Sentinel, when the decision's running total covers
the price. It does not attack, scout, build or expand.

## Boundary

The crate depends on `oxide-sim` and `chassis`, never on `oxide-bot`. It reads
only its seat's fog-honest `ObservationData` and emits ordinary
`PlayerCommand`s. It decides on its difficulty's interval and stays silent once
the match is decided, after its seat surrenders, or while it has no built
Foundry. Equal-distance choices are broken in the seat's home-relative frame, so
mirrored seats make mirrored choices.

## Selection

A seat runs this controller when its scenario `bot_config` names
`"controller": "opponent"`.

## Checkpoint and trace

`Checkpoint` holds only the seat. Restoring it checks that the seat is a
configured `oxide-opponent` bot in the bound scenario and world, and rebuilds
the profile and decision interval from the scenario.

`Opponent::act_traced` returns a `Trace` of the decision's tick, seat, bank,
committed spending, purchases and unit-order count. Traces are diagnostics only.
