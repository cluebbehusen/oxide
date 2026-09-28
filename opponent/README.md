# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. The [specification](../docs/oxide-opponent.md) is normative for
this crate, and the
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds its
working rules.

Each decision spends through one running total in a fixed precedence:
emergencies, then an affordable saving target, then workers, then production. A
seat with no Harvester alive or queued queues one even behind other work, from
protected scrap if it must. Each built Foundry works the four nearest live known
scrap nodes its ground can reach within twelve tiles, none shared with another
Foundry, and idle Foundries train Harvesters until there are two per worked
node. Paid sites nobody is building get the nearest free Harvester, and idle
Harvesters go to the reachable worked node with the fewest Harvesters. Every
idle producer then trains toward the army's needs. Difficulty caps the unit
orders one decision issues; purchases do not count against that allowance. It
does not yet attack, defend, scout or expand.

## Army composition

The seat remembers enemy units it has seen for 600 ticks, trusting them less as
they age and forgetting one when its last spot is in sight and empty. Enemy
buildings need no memory of its own: the observation keeps their ghosts.

From that knowledge and its own army, alive and queued, it sets a deficit for
each role: line fighters to match the enemy's ground army or two fifths of its
own, siege for known enemy defenses and by preference, anti-air to answer three
quarters of the enemy air it has seen (a seen enemy Airworks counts as air), and
air strikes by preference once it has or is saving for an Airworks. Each idle
producer trains for the most wanted role it can serve, or line units when
nothing is wanted, choosing among its units by coarse suitability: reach against
the enemy's usual reach, durability for the price, covering both enemy domains,
splash against clustered enemies, affordability at the seat's income, and
personality. Raiders, support units, scouts and transports are left to later
behavior. A role it needs but cannot train at all adds to the investment score
of the cheapest building that would let it.

## Investments and saving

Each decision scores its investments: a first Fabricator, Airworks and Crucible,
another Fabricator or Airworks when all of that kind are busy and income and
unprotected scrap could keep one more working, more Reclaimers, and Refinery
upgrades. Saturated harvesting, time, income, home depletion, army needs and
personality set the scores. The seat saves for one target at a time. It starts
saving only for an investment that scores well, keeps it while it still scores,
and switches only for one that scores clearly higher. Prerequisites come first:
saving for Airworks buys a Fabricator.

While saving, a share of the seat's estimated income is protected from ordinary
spending, up to the next purchase's price. Stance and greed set the share, and
visible hostile units near the base lower it. Income is estimated from the
bank's change between decisions plus the seat's own spending. Once the whole
uncommitted bank covers the next purchase, the seat places it on the first home
spot its knowledge allows, with the nearest free Harvester, or upgrades the
building. A purchase missing from the world at the next decision was rejected,
cancelled or refunded: the seat keeps the target, protects its full price again,
and skips that spot for a while.

Placement is checked against the fog-honest observation only: completed
prerequisites, explored ground, frames, known rock and scrap, known buildings, a
one-tile gap to the seat's own buildings, visible hostile ground units, its own
claims, and an open tile beside the footprint. Hidden units and unseen buildings
never change the verdict; the simulation re-checks on arrival.

## Boundary

The crate depends on `oxide-sim` and `chassis`, never on `oxide-bot`. It reads
only its seat's fog-honest `ObservationData` and its own order events, and emits
ordinary `PlayerCommand`s. Its seats also share one immutable `MapModel`, built
once per match from the scenario's public map: ground components and the
authored starts, home building spots, and home scrap. It decides on its
difficulty's interval and stays silent once the match is decided, after its seat
surrenders, or while it has no built Foundry. Equal-distance choices are broken
in a frame anchored on the seat's authored start, so mirrored seats make
mirrored choices.

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

`Checkpoint` holds the seat and what carries between its decisions: remembered
enemy units, footprints it recently failed to claim, its income sample, and its
saving target with the protected amount and any purchase awaiting confirmation.
The host saves the seat's `OwnEvents` beside it. Restoring it checks that the
seat is a configured `oxide-opponent` bot in the bound scenario and world and
that nothing it remembers is from a later tick, rebuilds the profile and
decision interval from the scenario, and takes the map model built from it. A
saved buffer over the cap does not load.

`Opponent::act_traced` returns a `Trace` of the decision's tick, seat, bank,
received own events, committed spending, purchases, unit-order count, allowance,
saving target with its next purchase, and protected scrap. Traces are
diagnostics only. `Opponent::protected_scrap` reports the protected amount to
hosts.
