# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. The [specification](../docs/oxide-opponent.md) is normative for
this crate, and the
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds its
working rules.

Each decision spends through one running total in a fixed precedence: defense
and other emergencies, then an affordable saving target, then workers, then
lifts, attacks and strikes, focus fire and scouting, then production. Missions
only give orders; what they need, such as a scout or carriers, production buys.
A seat with no worker alive or queued queues a Harvester even behind other work,
from protected scrap if it must. Each built Foundry works the four nearest live
known scrap nodes its ground can reach within twelve tiles, none shared with
another Foundry, and idle Foundries train workers until there are two
Harvesters' worth per worked node. Once a Fabricator stands, an Excavator, worth
two Harvesters, fills two open places when the seat can pay for it with scrap to
spare, less the greedier it is. A harvesting or idle worker away from home runs
back beside the nearest Foundry once known enemy fire reaches it. Paid sites
nobody is building get the nearest free worker. While no armed enemy in sight
stands near it and the seat has scrap to pay, the damaged building missing the
most value gets a free worker to weld it, two at most at once. Idle workers go
to the reachable worked node with the fewest workers, never one inside the reach
of known enemy weapons. Every idle producer then trains toward the army's needs.
Difficulty caps the unit orders one decision issues; purchases do not count
against that allowance.

## Defense

A visible enemy that can hit ground threatens the seat when it stands within
eight tiles of one of the seat's buildings, or within its weapon's reach if that
is longer, or when its shells could land where hostile shells are landing near
them. Threats group by the built Foundry each is nearest, and each group gets
one defend mission, the home Foundry's first. Only threats on or beside that
Foundry's ground count: one across water or a chasm is left to production, since
chasing it would stall every defender. The mission recruits free units that can
hit one of its threats, ground units only from that Foundry's ground, nearest
the Foundry first, until against ground and air attackers alike they are worth
half again what those attackers are. It sends them in one Hunt at the grounded
threat nearest the Foundry; against flyers alone they wait beside the building
nearest the raid, where anti-air reaches them, since the ground under a flyer
may be none they can stand on. It sends them again only when its goal moves more
than three tiles on the Foundry's ground or the mission re-engages, and a
defense that stops fighting while focused sends its units back to its goal
rather than after the retreating enemy. With no threat left the mission
recovers, lending its units to any other threatened Foundry, and after 120 quiet
ticks it lets them go where they stand. Once its own Foundries are answered, an
ally's Foundry under ground attack gets a defend mission of its own from the
units left free, which lends them back whenever the seat's own Foundries need
them; an ally's shortfall is never the seat's emergency.

A hostile shell landing near the seat's buildings that no enemy it knows of
could have fired comes from a gun out of sight. The built Foundry nearest the
impact then gets a defend mission that sends units that can hit ground, worth
half again one of the cheapest guns, toward where the gun probably stands: from
the impact toward the nearest hostile start, as far as artillery reaches, on the
Foundry's ground. Once the gun is in sight the ordinary defense takes over.

A defense that cannot recruit enough makes the decision an emergency: it skips
the saving purchase, trains no Harvesters beyond the one a seat without any
needs, and lets production spend protected scrap. A defense against a gun out of
sight never does, since static defenses cannot reach it. Missions own only units
that exist; production never works for a mission.

## Home reserve

Attacks, strikes, raids and lifts leave a reserve at home: units there, meaning
aircraft and units on the start's ground, worth enough against ground and
against aircraft for what could come. Against each, the reserve is nothing while
no enemy could reach home that way. Otherwise it is the stance's floor or its
share of the known enemy army that could, whichever is more, less the built
static defenses that cover it and the units already out defending. Turtle keeps
at least its minimum army and half again the known threat, Balanced half its
minimum and the threat, and Aggressive half the threat. Ground reaches home from
a hostile start connected by ground, or once an enemy carrier or Airworks is
seen; aircraft once enemy armed aircraft or an Airworks is seen. Offense takes
only units beyond the reserve, leaving those nearest home; a lift leaves its
weakest riders. Defense takes every unit regardless.

## Static defense

The seat guards its most valuable buildings (Foundries first, then tech and
production buildings, Extractors and Reclaimers) with Turrets, Bastions and Flak
Turrets a short gap from the building, on the side its threat comes from:
enemies in sight, else enemies it remembers, else known enemy buildings or the
nearest hostile start. Ground threats count only where ground connects them to
the building, except that a seat under the stance's minimum army also puts up a
Turret against enemy buildings and starts across a chasm, whose units could
land. Flak Turrets answer seen or remembered enemy aircraft or a known enemy
Airworks. Each site is an investment worth the approach it covers that no other
own weapon covers yet, by the building's value, by how sure the seat is of the
threat, and by personality: fortification for Turrets and Bastions,
fortification and support for Flak Turrets. A Bastion counts only ground the
seat's or an ally's buildings see. When the threat is only a guess from public
facts, equal sites go to the one nearest the building; otherwise to the one
nearest the threat. Sites go only on ground where one of the seat's Harvesters
stands to build them, and a site the simulation refused is skipped for a while.

Once the opening economy is up, a seat whose army is still under the stance's
minimum weighs a hostile start nearly like seen enemies, so most seats put up an
early Turret and fortified ones follow with a Bastion; otherwise public facts
alone move only very fortified seats. A defense that cannot recruit enough also
buys an emergency Turret, or Flak Turret against aircraft, beside a building
whose approach nothing covers when attackers stand near it, one unfinished at a
time.

Arrays watch the far part of those approaches: a site is worth the points out
there that nothing the seat owns sees yet, and its radar lets Bastions fire that
far. A Barricade goes a tile in front of a Turret or Bastion that has none, on
the side of the threat; it is refused at purchase when it would cut the start
off from a producer's exit, a worked scrap node or a hostile start on home
ground. Once a Fabricator stands, Scuttle Charges go a few tiles out on the
straight way in to a Foundry, spread so one blast does not set off the next.
Fortification weighs Barricades; fortification and guile weigh Arrays and
charges.

A Repair Bay goes up beside one of the two most valuable buildings where its
aura reaches the most missing value among the seat's wounded ground units and
damaged buildings that no Repair Bay reaches yet, weighted by support.

Defenses upgrade like Reclaimers do (Heavy Turret, Bulwark, Burst Flak, Deep
Array) once the next tier's prerequisite stands, worth the approach they cover,
or for an Array the far points it watches, by how sure the seat is of its threat
and weighted by fortification and greed, and never while an enemy in sight could
hit it, from its own reach or the defense's, while it is down at a fifth of its
health.

## Attack

An attack forms from the free line, siege and anti-air units at half health or
better beyond the home reserve when those that can hit ground are worth at least
the stance's minimum army and outweigh a reachable target's known local defense
by a margin set by difficulty; anti-air units recruited along the way escort the
army but do not count toward that. The target is the most valuable known enemy
building for its distance by ground, or a hostile start when none is known; a
better one replaces it only while the army gathers or recovers, and only when
clearly better. The army gathers at a rally near home toward the target's owner,
travels, and fights. It withdraws to the rally when the enemies it knows of
around it, remembered or seen, outweigh what it has left, and recovers there to
go again or disband. Having taken its target it pushes on to the next only while
strong enough for it. A target it withdrew from, could not reach, lost its army
to, or stood idle beside is skipped for a while. With several enemies, attacks
and strikes go after one rival's buildings first: the enemy pressing the seat
hardest, then the nearest, less the army it shows, with guile favoring a small
economy and a bonus for the owner of the current target so the seat does not
flip between enemies.

Members under 35 percent health leave between fights and run to the rally, and a
defense may take the attack's units in any phase but a fight. While an army that
could attack does not, or every producer sits idle, the margin falls step by
step toward even.

A free Tender joins an attack while it gathers or recovers, welds its wounded
while the army regroups there, and follows it otherwise.

At Veteran and Prime, an engaged mission focuses its fire: when every member
that can hit an enemy near it already reaches that enemy, they shoot the weakest
such enemy together, and keep that focus while it stays in reach. Nobody chases
a focus, and a member out of reach leaves the mission's fire unfocused.

## Lift

When the seat knows of enemy buildings or hostile starts and ground reaches none
of them, and the seat has at least the stance's minimum army to carry beyond the
home reserve, it needs lift. The Airworks then scores higher while the seat has
none, and production keeps enough Skyhooks, alive and queued, to carry what the
best landing needs, or the stance's minimum while none is known, but no more
than the riders at home fill. While an idle Airworks waits for the scrap to
train one and those riders already fill every carrier, other production waits
too, unless a defense is short.

A lift forms only from carriers and passengers that exist: free carriers that
are idle, empty and over open home ground, and free line and siege units at half
health or better on home ground beyond the home reserve, packed into them most
value per transport slot first. It needs those that can hit ground to be worth
the stance's minimum and to outweigh the target's known defense by the attack
margin. Carriers are loaded as the decision's orders allow until the loads sent
reach that need, and more are loaded on later decisions while those aboard or
walking fall short. The target is the most valuable known enemy building, or
hostile start not seen cleared, for its distance that no ground route reaches
and that has a landing: explored open ground on the target's island, set back
from it and clear of known fire, where every tile unloading could set a rider on
belongs to that island.

A free carrier hovering where no rider could reach it, such as over the Airworks
that trained it, first moves to open ground. Riders walk to their carriers and
board. Once none is still walking and no more were sent, or after a while, the
loaded carriers leave together in one order if everyone boarded or at least half
the need is aboard; a rider that stopped short is not sent again. Carriers fly
straight to the landing, or around known anti-air through a via-point when the
straight line crosses it. Each sets its riders down at the landing once there,
as the decision's orders allow, and emptied carriers fly home together the same
way. Riders still walking are stopped and let go; with less than half aboard the
lift sets everyone down and disbands. Landed riders hunt the target and, once no
one is aboard, fight on to the next target on the same island. Nothing brings
them home. A carrier that comes home still loaded sets its riders down and lets
them go. A target the lift lost its units to, or stood idle beside, is skipped
for a while. One lift runs at a time, and a defense may take its units only
while they board.

## Strikes

Free Buzzards, Darters, Condors and Moths at half health or better, beyond the
home reserve, strike the most valuable known enemy building or hostile start for
its distance, whether or not ground reaches it, when those that can hit ground
are worth the stance's minimum and outweigh the known anti-air reaching over the
target by the attack margin. They gather beside home on the side facing the
target, fly at it around known anti-air, and fight; having taken it they go on
to the next target they can, and otherwise fly home. They withdraw once the
known anti-air reaching them outweighs them. A target a strike withdrew from,
lost its aircraft to, or stood idle beside is skipped for a while. One strike
runs at a time, and a defense may take its aircraft while they gather or
withdraw.

## Raids

While no defense is under way, two or more free raiders of one kind at half
health or better beyond the home reserve set out on a raid. Scuttlers, up to
four, and ground-attack aircraft too few for a strike go after the enemy
Extractor or Foundry whose known defense they outweigh, least defended and then
nearest first, the aircraft flying around known anti-air. Sappers, up to four,
go after the most valuable known enemy building for its distance with little
known defense and blow it up. Scuttlers and aircraft turn back once one is lost
or badly hurt, once the known fire reaching them outweighs them, or once they
have been at the target a while; Sappers turn back only when outweighed on the
way. Raids skip a raided target for a while, which spaces them out, while
attacks, lifts and strikes may still go after it; one raid runs at a time.

The seat keeps two Scuttlers, alive or queued, once its income reaches a level
that falls with guile, and a Sapper for each known enemy defense that can hit
ground, up to three, once it has scrap to spare, less the more it leans on
siege. An attack on a target with such defenses takes free Sappers along while
it gathers or recovers, and once it fights sends each at the nearest such
defense on its ground, leaving them to it when the rest of the army moves on.

## Scouting

The seat keeps its hostile starts and expansion sites as scouting points and
remembers when it last saw each. Once one has gone unseen for a while, one scout
goes to the most valuable stale point, hostile starts first and then sites
nearer an enemy than home, and moves on to the next when it sees it. The scout
is a free Kestrel or Gnat, or else a Scuttler that can walk there. A seat
without either trains one: its air scout once it has an Airworks, a Scuttler
before. An air scout flies around known anti-air when the straight line crosses
it, and a point whose scout was lost on the way counts as seen, so the next
scout waits until it is stale again.

## Support

The seat keeps a Tender, alive or queued, for so much missing health among its
armed ground units, more the more it leans on support, up to two. A free idle
Tender welds the free wounded ground unit on its ground missing the most value;
one decision sends no two Tenders to the same patient.

## Army composition

The seat remembers enemy units it has seen for 600 ticks, trusting them less as
they age and forgetting one when its last spot is in sight and empty. Enemy
buildings need no memory of its own: the observation keeps their ghosts.

From that knowledge and its own army, alive and queued, it sets a deficit for
each role: line fighters to match three quarters of the enemy's ground army or
two fifths of its own, siege for known enemy defenses and by preference,
anti-air to answer three quarters of the enemy air it has seen (a seen enemy
Airworks counts as air), and air strikes by preference once it has an Airworks.
Ground units count only while they can reach an enemy, by ground or by lift once
an Airworks stands. While ground reaches no enemy, air strikes are wanted with
or without an Airworks, at least what a strike needs against the easiest known
target. The most wanted role goes first to the nearest idle producer that can
afford a unit for it, then the next; idle producers left with nothing wanted
train line units while ground units can reach an enemy. Each chooses the unit it
can afford now by coarse suitability: reach against the enemy's usual reach,
durability for the price, covering both enemy domains, splash against clustered
enemies, affordability at the seat's income, variety (a kind that already makes
up most of its role counts for less, so every kind of a role gets its turn), and
personality. Scouts, carriers, Tenders, Scuttlers and Sappers are bought only
for scouting, lift, support and raids. A role it needs but cannot train at all
adds to the investment score of the cheapest building that would let it.

## Investments and saving

Each decision scores its investments: a first Fabricator, Airworks and Crucible,
another Fabricator or Airworks when all of that kind are busy and income could
keep one more working, an expansion Foundry at a scrap field away from every
start, an Extractor on a free frame on its home ground, more Reclaimers (worth
more with no expansion left), static defenses, Repair Bays, and upgrades.
Saturated harvesting, time, income, home depletion, army needs, a needed lift
and personality set the scores.

An expansion site's value weighs the scrap it still holds and its free frames,
up with greed, against its ground distance from home, how much nearer a hostile
start it lies, and the danger the seat knows around it, down with greed; home
depletion raises every expansion. Sites off the seat's home ground, near another
seat's building or its own Foundry, or failed at every anchor are skipped. An
Extractor is worth more beside one of the seat's Foundries and less where it has
seen danger, and waits while an armed enemy in sight stands near its frame, as
one lost there does until the enemy leaves. The seat saves for one target at a
time. It starts saving only for an investment that scores well, keeps it while
it still scores, and switches only for one that scores clearly higher.
Prerequisites come first: saving for Airworks buys a Fabricator.

When it adopts a target a share of its bank is protected from ordinary spending,
and while it saves a share of its estimated income is added, up to the next
purchase's price. Stance and greed set the share, and visible hostile units near
the base lower it. Income is estimated from the bank's change between decisions
plus the seat's own spending. Once the whole uncommitted bank covers the next
purchase, the seat places it with the nearest free worker on the first spot its
knowledge allows (an expansion site's anchors, a frame, or otherwise a home
spot) or upgrades the building. If every spot is unexplored, the worker walks
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
gave up attacking, lifting to or striking, its income sample, and its saving
target with the protected amount and any purchase awaiting confirmation, its
missions with their phases, members, goals and focus, since when it has gone
without attacking, and when it last saw each scouting point. The host saves the
seat's `OwnEvents` beside it. Restoring it checks that the seat is a configured
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
