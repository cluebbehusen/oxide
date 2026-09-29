# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. The [specification](../docs/oxide-opponent.md) is normative for
this crate, and the
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds its
working rules.

Each decision spends through one running total in a fixed precedence: defense
and other emergencies, then an affordable saving target, then workers, then
lifts, attacks, focus fire and scouting, then production. Missions only give
orders; what they need, such as a scout or carriers, production buys. A seat
with no Harvester alive or queued queues one even behind other work, from
protected scrap if it must. Each built Foundry works the four nearest live known
scrap nodes its ground can reach within twelve tiles, none shared with another
Foundry, and idle Foundries train Harvesters until there are two per worked
node. Paid sites nobody is building get the nearest free Harvester, and idle
Harvesters go to the reachable worked node with the fewest Harvesters. Every
idle producer then trains toward the army's needs. Difficulty caps the unit
orders one decision issues; purchases do not count against that allowance.

## Defense

A visible enemy that can hit ground threatens the seat when it stands within
eight tiles of one of the seat's buildings, or within its weapon's reach if that
is longer. Threats group by the built Foundry each is nearest, and each group
gets one defend mission, the home Foundry's first. Only threats on or beside
that Foundry's ground count: one across water or a chasm is left to production,
since chasing it would stall every defender. The mission recruits free units
that can hit one of its threats, ground units only from that Foundry's ground,
nearest the Foundry first, until against ground and air attackers alike they are
worth half again what those attackers are. It sends them in one Hunt at the
grounded threat nearest the Foundry; against flyers alone they wait beside the
building nearest the raid, where anti-air reaches them, since the ground under a
flyer may be none they can stand on. It sends them again only when its goal
moves more than three tiles on the Foundry's ground or the mission re-engages,
and a defense that stops fighting while focused sends its units back to its goal
rather than after the retreating enemy. With no threat left the mission
recovers, lending its units to any other threatened Foundry, and after 120 quiet
ticks it lets them go where they stand.

A defense that cannot recruit enough makes the decision an emergency: it skips
the saving purchase and lets production spend protected scrap. Missions own only
units that exist; production never works for a mission.

## Attack

An attack forms from the free line, siege and anti-air units at half health or
better when those that can hit ground are worth at least the stance's minimum
army and outweigh a reachable target's known local defense by a margin set by
difficulty; anti-air units recruited along the way escort the army but do not
count toward that. The target is the most valuable known enemy building for its
distance by ground, or a hostile start when none is known; a better one replaces
it only while the army gathers or recovers, and only when clearly better. The
army gathers at a rally near home toward the target's owner, travels, and
fights. It withdraws to the rally when the enemies it knows of around it,
remembered or seen, outweigh what it has left, and recovers there to go again or
disband. Having taken its target it pushes on to the next only while strong
enough for it. A target it withdrew from, could not reach, lost its army to, or
stood idle beside is skipped for a while.

Members under 35 percent health leave between fights and run to the rally, and a
defense may take the attack's units in any phase but a fight. While an army that
could attack does not, or every producer sits idle, the margin falls step by
step toward even.

At Veteran and Prime, an engaged mission focuses its fire: when every member
that can hit an enemy near it already reaches that enemy, they shoot the weakest
such enemy together, and keep that focus while it stays in reach. Nobody chases
a focus, and a member out of reach leaves the mission's fire unfocused.

## Lift

When the seat knows of enemy buildings or hostile starts and ground reaches none
of them, it needs lift. The Airworks then scores higher while the seat has none,
and production keeps enough Skyhooks, alive and queued, to carry the stance's
minimum army, from one to four. While an idle Airworks waits for the scrap to
train one, other production waits too, unless a defense is short.

A lift forms only from carriers and passengers that exist: free carriers that
are idle, empty and over open home ground, and free line and siege units at half
health or better on home ground, packed into them. It needs those that can hit
ground to be worth the stance's minimum and to outweigh the target's known
defense by the attack margin. The target is the most valuable known enemy
building or hostile start for its distance that no ground route reaches and that
has a landing: explored open ground on the target's island, set back from it and
clear of known fire, where every tile unloading could set a rider on belongs to
that island.

A free carrier hovering where no rider could reach it, such as over the Airworks
that trained it, first moves to open ground. Riders walk to their carriers and
board. Once none is still walking, or after a while, the loaded carriers leave
together if everyone boarded or at least half the need is aboard; a rider that
stopped short is not sent again. Carriers fly straight to the landing, or around
known anti-air through a via-point when the straight line crosses it, and then
home the same way. Riders still walking are stopped and let go; with less than
half aboard the lift sets everyone down and disbands. Landed riders hunt the
target and, once no one is aboard, fight on to the next target on the same
island. Nothing brings them home. A carrier that comes home still loaded sets
its riders down and lets them go. A target the lift lost its units to, or stood
idle beside, is skipped for a while. One lift runs at a time, and a defense may
take its units only while they board.

## Scouting

The seat keeps its hostile starts and expansion sites as scouting points and
remembers when it last saw each. Once one has gone unseen for a while, one scout
goes to the most valuable stale point, hostile starts first and then sites
nearer an enemy than home, and moves on to the next when it sees it. The scout
is a free Kestrel or Gnat, or else a Scuttler that can walk there. A seat
without either trains one: its air scout once it has an Airworks, a Scuttler
before.

## Army composition

The seat remembers enemy units it has seen for 600 ticks, trusting them less as
they age and forgetting one when its last spot is in sight and empty. Enemy
buildings need no memory of its own: the observation keeps their ghosts.

From that knowledge and its own army, alive and queued, it sets a deficit for
each role: line fighters to match three quarters of the enemy's ground army or
two fifths of its own, siege for known enemy defenses and by preference,
anti-air to answer three quarters of the enemy air it has seen (a seen enemy
Airworks counts as air), and air strikes by preference once it has an Airworks.
Each idle producer trains for the most wanted role it can serve, or line units
when nothing is wanted, choosing the unit it can afford now by coarse
suitability: reach against the enemy's usual reach, durability for the price,
covering both enemy domains, splash against clustered enemies, affordability at
the seat's income, and personality. Raiders and support units are left to later
behavior; scouts and carriers are bought only for scouting and lift. A role it
needs but cannot train at all adds to the investment score of the cheapest
building that would let it.

## Investments and saving

Each decision scores its investments: a first Fabricator, Airworks and Crucible,
another Fabricator or Airworks when all of that kind are busy and income could
keep one more working, an expansion Foundry at a scrap field away from every
start, an Extractor on a free frame on its home ground, more Reclaimers (worth
more with no expansion left), and Refinery upgrades. Saturated harvesting, time,
income, home depletion, army needs, a needed lift and personality set the
scores.

An expansion site's value weighs the scrap it still holds and its free frames,
up with greed, against its ground distance from home, how much nearer a hostile
start it lies, and the danger the seat knows around it, down with greed; home
depletion raises every expansion. Sites off the seat's home ground, near another
seat's building or its own Foundry, or failed at every anchor are skipped. An
Extractor is worth more beside one of the seat's Foundries and less where it has
seen danger. The seat saves for one target at a time. It starts saving only for
an investment that scores well, keeps it while it still scores, and switches
only for one that scores clearly higher. Prerequisites come first: saving for
Airworks buys a Fabricator.

When it adopts a target a share of its bank is protected from ordinary spending,
and while it saves a share of its estimated income is added, up to the next
purchase's price. Stance and greed set the share, and visible hostile units near
the base lower it. Income is estimated from the bank's change between decisions
plus the seat's own spending. Once the whole uncommitted bank covers the next
purchase, the seat places it with the nearest free Harvester on the first spot
its knowledge allows (an expansion site's anchors, a frame, or otherwise a home
spot) or upgrades the building. If every spot is unexplored, the Harvester walks
toward one instead; with no spot left to place or explore, nothing is protected,
so a target that cannot stand anywhere never starves production. A purchase
missing from the world at the next decision was rejected, cancelled or refunded:
the seat keeps the target, protects its full price again, and skips that spot
for a while.

Placement is checked against the fog-honest observation only: completed
prerequisites, explored ground, frames, known rock and scrap, known buildings, a
one-tile gap to the seat's own buildings (except for Extractors, which sit where
the map put their frames), visible hostile ground units, its own claims, and an
open tile beside the footprint. Ground explored but out of sight is claimed as a
provisional scaffold. Hidden units and unseen buildings never change the
verdict; the simulation re-checks on arrival.

## Boundary

The crate depends on `oxide-sim` and `chassis`, never on `oxide-bot`. It reads
only its seat's fog-honest `ObservationData` and its own order events, and emits
ordinary `PlayerCommand`s. Its seats also share one immutable `MapModel`, built
once per match from the scenario's public map: ground components, the authored
starts and teams, ground distance from every start, home building spots and
scrap, and expansion sites. Distances between buildings and units are measured
between whole footprints, so they stay equal for mirrored seats. It decides on
its difficulty's interval and stays silent once the match is decided, after its
seat surrenders, or while it has no built Foundry. Equal-distance choices are
broken in a frame anchored on the seat's authored start, so mirrored seats make
mirrored choices.

## Own events

Each decision also receives the seat's own `OrderStalled` and `CommandRejected`
events since its previous decision, oldest first. `OwnEvents` holds them: the
host keeps one per seat beside the controller and calls `record` after every
tick, and a decision takes them all. A buffer holds at most 64 events and drops
the oldest first. A tick without a decision leaves the buffer untouched. A
rejection only holds back that decision's income sample; the trace reports the
events.

## Selection

A seat runs this controller when its scenario `bot_config` names
`"controller": "opponent"`. `oxide-kit` hosts it next to `oxide-bot`, so live,
headless, saved and recovered sessions build it from the same scenario data.

## Checkpoint and trace

`Checkpoint` holds the seat and what carries between its decisions: remembered
enemy units, footprints it recently failed to claim, enemy buildings it recently
gave up attacking or lifting to, its income sample, and its saving target with
the protected amount and any purchase awaiting confirmation, its missions with
their phases, members, goals and focus, since when it has gone without
attacking, and when it last saw each scouting point. The host saves the seat's
`OwnEvents` beside it. Restoring it checks that the seat is a configured
`oxide-opponent` bot in the bound scenario and world and that nothing it
remembers is from a later tick or off the map, rebuilds the profile and decision
interval from the scenario, and takes the map model built from it. A saved
buffer over the cap does not load.

`Opponent::act_traced` returns a `Trace` of the decision's tick, seat, bank,
received own events, committed spending, purchases, unit-order count, allowance,
saving target with its next purchase, protected scrap, and missions. Traces are
diagnostics only. `Opponent::protected_scrap` and `Opponent::missions` report
the protected amount and each mission's phase, when it began and the timeout it
should end within to hosts.
