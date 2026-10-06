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
from protected scrap if it must. A live known scrap node's route is the shortest
ground route from the seat's start, followed back until it comes home to one of
the seat's Foundries; it is clear while it crosses neither the reach of known
enemy weapons (a site in sight cannot fire yet) nor the surroundings of an enemy
Foundry, known or presumed. Each node with a clear route belongs to the nearest
built Foundry whose ground reaches it, and is worked when a Harvester hauling
from there repays its price within a horizon: three minutes for Turtle, two for
Balanced and one and a half for Aggressive, stretched by up to half again the
greedier the seat is. A worked node wants as many Harvesters as free tiles
beside it hold and its remaining scrap repays, all of them for Turtle and three
quarters for Balanced and half for Aggressive, more the greedier the seat is,
and ready Foundries train workers until every worked node has its crew. Until
the army reaches the stance's minimum, workers that would cost more than the
army wait for army production and take only the Foundries it leaves. A producer
is ready when its queue would run out before the next decision, so it queues its
next unit as the last one finishes rather than after it stands empty. Once a
Fabricator stands, an Excavator, worth two Harvesters, fills two open places
when the seat can pay for it with scrap to spare, less the greedier it is. A
harvesting or idle worker away from home runs back beside the nearest Foundry
once known enemy fire reaches it. Paid sites nobody is building get the nearest
free worker. While no armed enemy in sight stands near it and the seat has scrap
to pay, each damaged building nobody welds yet gets the nearest free worker,
most missing value first. Idle workers go to the reachable worked node with the
most places open, or with none worked to the nearest node with a clear route,
never one inside the reach of known enemy weapons; with neither they wait at
home. A worker harvesting a node whose route has turned dangerous is sent
elsewhere the same way, or home. A worker left with nowhere to go first delivers
any scrap it carries to a Foundry on its ground. Every ready producer then
trains toward the army's needs. Difficulty caps the unit orders one decision
issues; purchases do not count against that allowance. Through its first four
minutes, Scrapheap trains an army only when something it has seen calls for one.

## Defense

A visible enemy that can hit ground threatens the seat when it stands within
eight tiles of one of the seat's buildings, or within its weapon's reach if that
is longer, or when its shells could land where hostile shells are landing near
them. So does a known enemy building, in sight or remembered, whose shells could
land there, while the units that can hit ground the defense holds or could take
outweigh the known defense around the tile beside it nearest the Foundry by half
again; defenders go to that tile. A battery among stronger defenses is left to
production. Threats group by the built Foundry each is nearest, and each group
gets one defend mission, the home Foundry's first. Only threats on or beside
that Foundry's ground count: one across water or a chasm is left to production,
since chasing it would stall every defender. The mission recruits free units
that can hit one of its threats, ground units only from that Foundry's ground,
nearest the Foundry first, until against ground and air attackers alike they are
worth half again what those attackers are. It sends them in one Hunt at the
grounded threat nearest the Foundry, except that artillery is reached rather
than hunted, since a Hunt stops for whatever comes first, such as a spotter
overhead, while the guns shell the defenders from beyond their reach: with a gun
in sight on the Foundry's ground, those that can hit ground attack the nearest
one and are sent again when it falls. Against flyers alone they wait beside the
building nearest the raid, where anti-air reaches them, since the ground under a
flyer may be none they can stand on. It sends them again only when its goal
moves more than three tiles on the Foundry's ground or the mission re-engages,
and a defense that stops fighting while focused sends its units back to its goal
rather than after the retreating enemy. With no threat left the mission
recovers, lending its units to any other threatened Foundry, and after 120 quiet
ticks it lets them go where they stand. Once its own Foundries are answered, an
ally's Foundry under ground attack gets a defend mission of its own from the
units left free, which lends them back whenever the seat's own Foundries need
them; an ally's shortfall is never the seat's emergency.

A hostile shell landing near the seat's buildings that no enemy it knows of
could have fired comes from a gun out of sight. Once the built Foundry nearest
the impact has no threat in sight, it gets a defend mission that sends units
that can hit ground, worth half again one of the cheapest guns, toward where the
gun probably stands: from the impact toward the nearest hostile start, as far as
artillery reaches, on the Foundry's ground. They Advance there, so they do not
stop for anything on the way. Once the gun is in sight the ordinary defense
takes over.

A defense that cannot recruit enough makes the decision an emergency: it skips
the saving purchase, trains no Harvesters beyond the one a seat without any
needs, and lets production spend protected scrap. Ready producers the army's
roles leave idle then each train the costliest armed unit the scrap on hand buys
that can reach and hit a Foundry whose defense is short, until what the decision
queued makes up each Foundry's shortfall, so a seat too poor for its line unit
or a gun still fields what it can afford; a ground unit counts only for a
Foundry on its producer's ground. Shelling, from a building or a gun out of
sight, never makes an emergency, since static defenses cannot reach the guns.
Missions own only units that exist; production never works for a mission.

## Home reserve

Attacks, strikes, raids and lifts leave a reserve at home: units there, meaning
aircraft and units on the start's ground, worth enough against ground and
against aircraft for what could come. Against each, the reserve is nothing while
no enemy could reach home that way. Otherwise it is the stance's floor or its
share of the known enemy army that could, whichever is more, less the built
static defenses on the start's ground that cover it and the units already out
defending. Turtle keeps at least its minimum army and half again the known
threat, Balanced half its minimum and the threat, and Aggressive half the
threat. Ground reaches home from a hostile start connected by ground, or once an
enemy carrier or Airworks is seen; aircraft once enemy armed aircraft or an
Airworks is seen. Offense takes only units beyond the reserve, leaving those
nearest home; a lift leaves its weakest riders. Defense takes every unit
regardless.

## Static defense

The seat guards each base, a Foundry it started or founded on an expansion site
with every other building but defenses nearest it, and each Extractor more than
eight tiles from every Foundry on its own. Turrets, Bastions and Flak Turrets go
a short gap from one of those buildings, on the side the threat comes from and
in front of the base's edge, the building furthest toward it: enemies in sight,
else enemies it remembers, else known enemy buildings or the nearest hostile
start. Where every ground way in from a hostile start runs through a cut, at
most three gates a few units abreast each, together narrower than their distance
from the start and clear of Extractor frames and expansion sites, beyond the
base's buildings and with the threat beyond it, the guns stand on the home side
of each gate instead, holding the narrowest such cut; no building stands in a
gate. Ground threats count only where ground connects them to the building,
except that a seat under the stance's minimum army also puts up a Turret against
enemy buildings and starts across a chasm, whose units could land. Flak Turrets
answer seen or remembered enemy aircraft or a known enemy Airworks. A gun holds
off the army scrap of Sentinels that would match its damage and health in a
fight, at its current tier, a splash shell counting several hits when the armed
enemies behind that approach gather in clumps, and none of the share of the
threat that can hit it from beyond its reach or from inside its minimum range.
Guns hold a threat once together they hold off its value divided by the margin
an attack is assumed to bring over a defense, the same at every difficulty; the
threat is the armed enemies known behind the approach, on ground connected to
the building unless they could only land, and at least an army at the stance's
minimum. Each site is an investment worth the shortfall it closes at the points
of the approach it covers, from the edge or each gate outward, at most what the
gun holds off at each, per scrap of its price, by the building's value, by how
sure the seat is of the threat, and by personality: fortification for Turrets
and Bastions, fortification and support for Flak Turrets. A seat under the
stance's minimum army with no Turret puts one up before any tech, whatever its
fortification: one gun holds an early rush that a tech building still going up
would not. A Bastion counts only ground the seat's or an ally's buildings see.
When the threat is only a guess from public facts, equal sites go to the one
nearest the building; otherwise to the one nearest the threat. Sites go only on
ground where one of the seat's Harvesters stands to build them, and a site the
simulation refused is skipped for a while.

Once the opening economy is up, a seat whose army is still under the stance's
minimum weighs a hostile start nearly like seen enemies, so most seats put up an
early Turret and fortified ones follow with more guns, Turrets near the building
and Bastions once the near approach holds; otherwise public facts alone move
only very fortified seats. A defense that cannot recruit enough also buys an
emergency Turret, or Flak Turret against aircraft, beside each building, inside
the base or not, whose approach nothing covers when attackers stand near it, one
unfinished at a time beside each.

Arrays watch the far part of those approaches: a site is worth the points out
there that nothing the seat owns sees yet, and its radar lets Bastions fire that
far. A Barricade goes a tile in front of a Turret or Bastion that has none, on
the side of the threat; it is refused at purchase when it would cut the start
off from a producer's exit, a worked scrap node or a hostile start on home
ground. Guns, Arrays, Barricades and Repair Bays stand off the lanes of the
base's layout. Once a Fabricator stands, Scuttle Charges mine the straight way
in to each base and Extractor on its own: the lanes within a band about a blast
wide, filled from a few tiles beyond the edge toward the threat, or across the
gates of the cut it holds, each by its width and the share of its way on the
guns leave open. On ground no Foundry lays out they may go anywhere. A buried
charge blocks nothing and a blast never sets off the seat's own charges, so they
stand side by side. A field holds enough charges to deal the health of the
threat along the way divided by that margin, in the share of the approach the
guns leave open, one body to a blast, the threat counting at least an army of
Sentinels at the stance's minimum. Fortification weighs Barricades;
fortification and guile weigh Arrays and charges.

A Repair Bay goes up beside a guarded base's building where its aura reaches the
most missing value among the seat's wounded ground units and damaged buildings
that no Repair Bay reaches yet, weighted by support.

Defenses upgrade like Reclaimers do (Heavy Turret, Bulwark, Burst Flak, Deep
Array) once the next tier's prerequisite stands, worth how far the approach they
cover still falls short of holding, or for an Array the far points it watches,
by how sure the seat is of its threat and weighted by fortification and greed,
and never while an enemy in sight could hit it, from its own reach or the
defense's, while it is down at a fifth of its health.

## Attack

Attacks, lifts, strikes and raids each run side by side: each decision advances
every mission of a kind, oldest first, then forms another while the free units
beyond the home reserve meet the need of the best target no mission of that kind
holds. Each takes only the force its own target needs, and missions of different
kinds may still go after one target.

An attack forms from the free line, siege and anti-air units at half health or
better beyond the home reserve when those that can hit ground are worth at least
the stance's minimum army and outweigh a reachable target's known local defense
by a margin set by difficulty; anti-air units recruited along the way escort the
army but do not count toward that. Known defense counts remembered armed units
by confidence and known enemy buildings that fire on ground at their price with
every upgrade they reached, discounted by missing health; a building's weapons
reach as far as its current tier's do. The target is the most valuable known
enemy building for its distance by ground, or a hostile start when none is
known. Veteran and Prime count the known defense around a target like more
distance, so they go after the weakest valuable target; Scrapheap goes for a
Foundry whenever it knows one, however well guarded; a better one replaces it
only while the army gathers or recovers, and only when clearly better. The army
gathers at a rally near home toward the target's owner, travels, and fights. It
withdraws to the rally when the enemies it knows of around it, remembered or
seen, outweigh what it has left, and recovers there to go again or disband.
Having taken its target it pushes on to the next only while strong enough for
it. A target it withdrew from, could not reach, lost its army to, or stood idle
beside is skipped for a while. With several enemies, attacks and strikes go
after one rival's buildings first: the enemy pressing the seat hardest, then the
nearest, less the army it shows, with guile favoring a small economy and a bonus
for the owner of the oldest attack or strike's target so the seat does not flip
between enemies. The rival is chosen once per decision, after defense.

Members under 35 percent health leave between fights and run to the rally, and a
defense may take the attack's units in any phase but a fight. While a free army
that could attack does not, or every producer sits idle, the margin for the next
attack falls step by step toward even.

Free Tenders join an attack while it gathers or recovers, one for so much
missing health among its members and at least one, weld its wounded while the
army regroups there and the seat has scrap to pay, and follow it otherwise.

At Veteran and Prime, an engaged mission focuses its fire: when every member
that can hit an enemy near it already reaches that enemy in a straight line that
terrain does not stop, and every member on the ground stands on that enemy's
ground, they shoot the weakest such enemy together, and keep that focus while it
stays in reach. Nobody chases a focus or seeks a way round to it, and a member
out of reach leaves the mission's fire unfocused.

## Lift

When the seat knows of enemy buildings or hostile starts and ground reaches none
of them, and the seat has at least the stance's minimum army to carry beyond the
home reserve, it needs lift. The Airworks then scores higher while the seat has
none, and production keeps enough Skyhooks, alive and queued, to carry what the
best landing needs, or the stance's minimum while none is known, with the free
riders at home packed as a lift would pack them, but no more than those riders
fill. While a ready Airworks waits for the scrap to train one and some of those
riders have no room, other production waits too, unless a defense is short.

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
one is aboard and every emptied carrier is on its way home, fight on to the next
target on the same island. Nothing brings them home. A carrier that comes home
still loaded sets its riders down and lets them go. A target the lift lost its
units to, or stood idle beside, is skipped for a while. A lift never grows past
the mission member cap, and a defense may take its units only while they board.

## Strikes

Free Buzzards, Darters, Condors and Moths at half health or better, beyond the
home reserve, strike the most valuable known enemy building or hostile start for
its distance, whether or not ground reaches it, when those that can hit ground
are worth the stance's minimum and outweigh the known anti-air reaching over the
target by the attack margin. They gather beside home on the side facing the
target, fly at it around known anti-air, and fight; having taken it they go on
to the next target they can, and otherwise fly home. They withdraw once the
known anti-air reaching them outweighs them. A target a strike withdrew from,
lost its aircraft to, or stood idle beside is skipped for a while. A defense may
take a strike's aircraft while they gather or withdraw.

## Raids

While no defense is under way, free raiders of one kind at half health or better
beyond the home reserve set out on a raid, those nearest the target until they
are worth what it needs. Scuttlers and ground-attack aircraft too few for a
strike go after the enemy Extractor or Foundry whose known defense they outweigh
by the attack margin, least defended and then nearest first, Scuttlers only
where no known, built enemy building's ground fire reaches and the known guard
is light (at most a quarter of the stance's minimum army), the aircraft flying
around known anti-air. Sappers go after the most valuable known enemy building
for its distance with little known defense that they are enough to blow up.
Scuttlers and aircraft turn back once one is badly hurt, once the known fire
reaching them outweighs them, or once they have been at the target a while;
Sappers turn back only when outweighed on the way. Raids skip a raided target
for a while, which spaces them out, while attacks, lifts and strikes may still
go after it.

Once its income reaches a level that falls with guile, the seat keeps the
Scuttlers, alive or queued and out on no other mission, that a raid on the least
defended known harvest line no enemy gun guards and only a light guard holds
needs. Once it has scrap to spare, less the more it leans on siege, it keeps a
Sapper for each known enemy defense, in sight or remembered, that can hit ground
around the targets of attacks under way and of the next attack. An attack on a
target with such defenses takes a free Sapper along for each while it gathers or
recovers, and once it fights sends each at the nearest such defense on its
ground, leaving them to it when the rest of the army moves on.

## Scouting

The seat keeps its hostile starts and expansion sites as scouting points and
remembers when it last saw each. Each point unseen for a while draws its own
scout, the most valuable first: hostile starts, then sites nearer an enemy than
home. A scout moves on to the next point no other scout holds when it sees its
own. Scouts are free Kestrels or Gnats, or else Scuttlers that can walk there.
For each stale point no scout could take, the seat trains one that could reach
it: its air scout once it has an Airworks, a Scuttler before. It trains none
while no mission could take another. An air scout flies around known anti-air
when the straight line crosses it, and a point whose scout was lost on the way
counts as seen, so the next scout waits until it is stale again.

## Support

The seat keeps a Tender, alive or queued, for so much missing health among its
armed ground units, more the more it leans on support. While the seat has scrap
to pay for a weld, a free idle Tender welds the free wounded ground unit on its
ground missing the most value; one decision sends no two Tenders to the same
patient.

## Army composition

The seat remembers every enemy unit it has seen for 600 ticks, trusting them
less as they age and forgetting one when its last spot is in sight and empty.
Enemy buildings need no memory of its own: the observation keeps their ghosts.

From that knowledge and its own army, alive and queued, it sets a deficit for
each role: line fighters to match three quarters of the enemy's ground army or
two fifths of its own, siege for known enemy defenses and, unless the army goes
only by lift, for a share of its own army behind that line (half for Turtle and
Balanced seats, a fifth for Aggressive ones, which attack early with small
armies; more or less with its siege preference), anti-air to answer three
quarters of the enemy air it has seen (a seen enemy Airworks counts as air), and
air strikes by preference once it has an Airworks. Ground units count only while
they can reach an enemy, by ground or by lift once an Airworks stands; before
then, known enemy ground units on the seat's own ground still call for line
units worth three quarters of them. While ground reaches no enemy, air strikes
are wanted with or without an Airworks, at least what a strike needs against the
easiest known target the seat has not given up on. The most wanted role goes
first to the nearest ready producer that can afford a unit for it, then the
next; ready producers left with nothing wanted train, while ground units can
reach an enemy, for whichever of line and siege they serve is furthest below its
share, line alone when the army goes only by lift, unless another producer
serves a wanted role: the scrap then waits for that producer. While the seat
saves for a unit, the producers that train it wait for it and no other producer
trains a cheaper unit of its role. Each chooses the unit it can afford now by
coarse suitability: reach against the enemy's usual reach, what its role is for
at the price (firepower for siege, durability for the rest), covering both enemy
domains, splash against clustered enemies, affordability at the seat's income,
variety (a kind that already makes up most of its role counts for less, so every
kind of a role gets its turn), and personality. Scouts, carriers, Tenders,
Scuttlers and Sappers are bought only for scouting, lift, support and raids. A
role it needs but cannot train at all adds to the investment score of the
cheapest building that would let it, and a role it can train adds to a producer
it lacks whose unit would suit it better, as much as saving for that unit is
worth.

## Investments and saving

Each decision scores its investments: a first Fabricator, Airworks and Crucible,
another Fabricator, Airworks, Crucible or home Foundry when every one of that
kind is working and either a unit the seat has saved enough for waits on it, or
an army role it trains is still wanted and the income the working producers
leave unspent could keep one more as busy (carriers, Tenders, Scuttlers and
Sappers never call for one), an expansion Foundry at a scrap field away from
every start, an Extractor on a free frame on its home ground, more Reclaimers
(worth more with no expansion left, and each one owned weighing less against the
next as the scrap around home runs out), static defenses, Repair Bays, upgrades,
and the unit a wanted role suits best when it costs more than the scrap on hand
and the role has a cheaper unit production would buy instead, worth more the
more of two of it the role lacks. Saturated harvesting, time, income, home
depletion, army needs, a needed lift and personality set the scores.

An expansion site's value weighs the scrap it still holds and its free frames,
up with greed, against its ground distance from home, how much nearer a hostile
start it lies, and the danger the seat knows around it, down with greed; home
depletion raises every expansion. Sites off the seat's home ground, near another
seat's building or its own Foundry, or failed at every anchor are skipped. An
Extractor is worth more beside one of the seat's Foundries and less where it has
seen danger, and waits while an armed enemy in sight stands near its frame, as
one lost there does until the enemy leaves. The seat saves for one target at a
time. It starts saving only for an investment that scores well and could stand
somewhere it knows of or could look, keeps it while it still scores, and
switches only for one that scores clearly higher. Prerequisites come first:
saving for Airworks buys a Fabricator.

When it adopts a target a share of its bank is protected from ordinary spending,
and while it saves a share of its estimated income is added, up to the next
purchase's price. Stance and greed set the share, and visible hostile units near
the base lower it. Income is estimated from the bank's change between decisions
plus the seat's own spending. Once the whole uncommitted bank covers the next
purchase, the seat places it with the nearest free worker on the first spot its
knowledge allows (an expansion site's anchors, a frame, or otherwise a spot
beside one of its Foundries), upgrades the building, or trains the unit at the
nearest ready producer. Every Foundry lays out the ground around it in blocks
four tiles a side with a lane one tile wide between them; its own block is the
Foundry and the ring around it. Another Foundry takes the centre of an empty
block, a two-by-two building a corner of a block, touching two lanes, and a
smaller one a corner's tiles beside a lane, so buildings pack side by side and
every one has a lane beside it. Once those run out on a ground, any place off
the lanes follows, so cramped ground still takes whatever fits. Spots are open
ground clear of frames and starting scrap, taken nearest first: every Foundry's
spots at one gap, the start's first, before any at the next, out to the edge of
its ground, each in the layout of the Foundry nearest it, and while any Foundry
stands on ground one of the seat's workers stands on, only beside those. A spot
that would leave the building, a site still going up or a producer without a way
out, or cut the seat off from a worked scrap node or a hostile start, is skipped
for a while. So is a spot for anything but a defense where a gun the seat knows
of or an armed enemy it remembers could hit it, or where it lately took damage.
If the first spot it could use is unexplored, the worker walks toward it
instead; with no spot left to place or explore, nothing is protected, so a
target that cannot stand anywhere never starves production. A purchase missing
from the world at the next decision was rejected, cancelled or refunded: the
seat keeps the target, protects its full price again, and skips that spot for a
while.

Placement is checked against the fog-honest observation only: completed
prerequisites, explored ground, frames, known rock and scrap, known buildings, a
one-tile gap to the seat's own buildings, or for a building packed into a block
only to its Foundries' rings (Extractors sit where the map put their frames, and
the seat's own buried charges keep nothing away), visible hostile ground units,
its own claims, and an open tile beside the footprint. Ground explored but out
of sight is claimed as a provisional scaffold. Hidden units and unseen buildings
never change the verdict; the simulation re-checks on arrival.

## Boundary

The crate depends on `oxide-sim` and `chassis`, never on `oxide-bot`. It reads
only its seat's fog-honest `ObservationData` and its own order events, and emits
ordinary `PlayerCommand`s. Its seats also share one immutable `MapModel`, built
once per match from the scenario's public map: ground components, the authored
starts and teams, ground distance from every start, where building spots may go,
home scrap, and expansion sites. Distances between buildings and units are
measured between whole footprints, so they stay equal for mirrored seats. It
decides on its difficulty's interval and stays silent once the match is decided,
after its seat surrenders, or while it has no built Foundry. Equal-distance
choices are broken in a frame anchored on the seat's authored start, so mirrored
seats make mirrored choices.

## Own events

Each decision also receives the seat's own `OrderStalled` and `CommandRejected`
events since its previous decision, oldest first. `OwnEvents` holds them: the
host keeps one per seat beside the controller and calls `record` after every
tick, and a decision takes them all. A repeat of a unit's stall for the same
reason, or of a rejection for the same reason, replaces the earlier one in its
place; a buffer holds at most 1,024 events and drops the oldest first. A tick
without a decision leaves the buffer untouched. A rejection holds back that
decision's income sample. A unit whose order stalled for want of a route sits
out orders, and joins no new mission or work, until it moves off where it
stopped or 600 ticks pass. The trace reports the events.

## Selection

A seat runs this controller when its scenario `bot_config` names
`"controller": "opponent"`. `oxide-kit` hosts it next to `oxide-bot`, so live,
headless, saved and recovered sessions build it from the same scenario data.

## Checkpoint and trace

`Checkpoint` holds the seat and what carries between its decisions: remembered
enemy units, footprints it recently failed to claim, enemy buildings it recently
gave up attacking, lifting to or striking, its income sample, and its saving
target with the protected amount and any purchase awaiting confirmation, its
missions with their phases, members, goals and focus, since when a free army
that could attack has not, and when it last saw each scouting point. The host
saves the seat's `OwnEvents` beside it. Restoring it checks that the seat is a
configured `oxide-opponent` bot in the bound scenario and world and that nothing
it remembers is from a later tick or off the map, rebuilds the profile and
decision interval from the scenario, and takes the map model built from it. A
saved buffer over the cap does not load.

`Opponent::act_traced` returns a `Trace` of the decision's tick, seat, bank,
received own events, committed spending, purchases, unit-order count, allowance,
saving target with its next purchase, protected scrap, and missions. Traces are
diagnostics only. `Opponent::protected_scrap` and `Opponent::missions` report
the protected amount and each mission's phase, when it began and the timeout it
should end within to hosts. `Opponent::launches` reports the attacks the last
decision launched, each with the known defense, margin, need and strength sent
it was judged by and the units sent; it is never saved and no decision reads it.
