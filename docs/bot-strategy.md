# Scripted bot strategy

This document defines the strategic model toward which Oxide's player-facing
rules-based bot should evolve. It is normative: the current implementation only
partially realizes it. Architecture documents describe what the code does today;
this document guides decisions about what the bot should learn to do next.

The goal is a credible opponent that appears to play the whole game. It should
gather information, form and revise beliefs, invest limited resources, execute
recognizable plans, and respond to what its opponent actually does. Winning is
important, but a high win rate cannot substitute for coherent, legible play.

## Guiding principle

The bot is a constrained capital allocator. Using public map knowledge and
fog-honest observations, it continually compares economic, defensive,
technological, reconnaissance, support, and offensive opportunities. It invests
real scrap, production time, builder attention, and units according to expected
value, urgency, confidence, risk, time to impact, and opportunity cost.

Personality influences this process twice. It biases which domains look most
attractive at the strategic allocation layer, then shapes the choices made
inside the selected domain: what kind of expansion to prefer, how to fortify,
which force composition to field, how much to scout, when to commit, and how
readily to raid or withdraw. Personality never grants or removes access to a
domain, unit, command, fact, or game rule. Strong evidence may outweigh a
preference, but otherwise two personalities should make recognizably different
decisions from the same legal possibilities.

## The strategic loop

The bot should repeatedly move through one deterministic loop:

```text
Observe -> Remember -> Forecast -> Allocate -> Plan -> Commit -> Evaluate
```

1. **Observe.** Read only the immutable public briefing and the current
   fog-honest observation.
2. **Remember.** Retain timestamped facts, prior outcomes, losses, and
   unresolved suspicions in controller-local deterministic memory.
3. **Forecast.** Estimate threats, opportunities, future income, production
   throughput, travel time, and likely counters. Preserve uncertainty instead of
   turning an old sighting into a current fact.
4. **Allocate.** Compare demands across domains and protect resources for the
   most valuable compatible portfolio of work.
5. **Plan.** Let each funded domain choose its own target, scale, composition,
   location, and sequence from the evidence and resources available to it.
6. **Commit.** Convert a revisable proposal into bounded ownership of exact
   builders, factories, units, sites, and current scrap.
7. **Evaluate.** Observe the result, update memory, release completed or failed
   claims, and reconsider the next allocation without thrashing.

This is a continuing loop, not a scripted build order followed by generic unit
production. Openings may establish safe defaults, but the bot must increasingly
act from the match it is playing.

## Honest knowledge and learning

The bot may begin with the same authored information available to a person:
static terrain, starting positions, Extractor frames, initial scrap placement,
teams, and other explicitly public facts. These are priors, not proof of current
ownership, current resource amounts, or present enemy activity.

Within a match, learning means updating deterministic beliefs from legal
evidence. Examples include:

- remembering where and when an enemy force was seen;
- inferring an air threat from observed aircraft or production;
- recognizing that a harvest region repeatedly caused losses;
- noticing weak anti-air coverage and considering an air investment;
- recognizing a fortified opponent and valuing siege, expansion, or another
  approach more highly;
- tracking whether a prior route, attack, defense, or investment succeeded.

Memory needs confidence, age, and invalidation rules. A remembered contact can
motivate reconnaissance or preparedness, but only current evidence can justify
actions that require current certainty. Learning must remain controller-local,
seeded, replayable, and reconstructable from the recorded match prefix. It does
not imply machine learning, cross-match mutable state, or access to omniscient
views.

## Allocation across domains

Each domain should present a structured investment case rather than claiming
scrap merely because it has an available action. A useful case includes:

- the opportunity or threat being addressed;
- the evidence supporting it and the confidence in that evidence;
- urgency and expected time to impact;
- current scrap plus a conservative forecast of completed recurring income;
- builder, factory, queue, and unit capacity required;
- minimum credible commitment and useful ways to scale beyond it;
- risk, reversibility, and the consequence of doing nothing;
- conflicts with existing commitments and protected reserves.

The allocator does not need one universal magic score. It does need comparable,
named considerations and deterministic precedence so that competing choices are
explainable and testable. Immediate survival and already-paid obligations may
form constraints around the choice; the remaining capacity should be assigned to
the best portfolio rather than drained by whichever subsystem runs first.

The allocator may fund compatible work concurrently. Scouting can support an
attack, a defensive position can protect an expansion, and an Airworks can serve
an operation while another investment is still being built. Shared scrap,
builders, factories, routes, and units must nevertheless have one explicit owner
at a time.

## Decisions within a domain

Receiving an allocation does not predetermine the answer inside a domain. Each
domain applies its own mechanics and tactical knowledge.

- **Economy** chooses among workers, Extractors, Reclaimers, Foundries,
  upgrades, and production capacity based on reachable opportunity, safety,
  saturation, payback time, and expected future demand.
- **Defense** chooses what deserves protection, which hostile approaches are
  credible, which defensive role answers the threat, and where that structure
  can produce useful coverage without severing routes.
- **Technology** compares the cost and delay of new capabilities with what the
  current match is likely to reward. Unlocking a tier is not itself a reason to
  buy every unit in it.
- **Reconnaissance** chooses what uncertainty is strategically expensive, what
  information would change a decision, and the cheapest safe way to obtain it.
- **Offense** chooses a doctrine and objective, then derives an attainable force
  package from observed defenses, available technology, existing units,
  production throughput, protected capital, and a fixed preparation horizon.
- **Support** responds to concrete forces, damage, allies, routes, and planned
  operations rather than filling an isolated quota.

Domain logic may use minimum viable commitments. A bomber operation that cannot
reconnoiter or survive known anti-air is not a smaller valid bomber operation.
Once the minimum is satisfied, scale should come from marginal value and
available capacity rather than a controller-only ceiling.

## Personality at both layers

The six seeded traits should leave evidence at both strategic and domain levels:

| Trait         | Strategic allocation tendency                          | Within-domain expression                                                               |
| ------------- | ------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| Air           | Values air infrastructure and air opportunities sooner | Favors air screening, interception, bombing, and transport when those roles fit        |
| Siege         | Values patient investment against fixed positions      | Favors standoff force, suppression, staging, and deliberate objective selection        |
| Support       | Values force preservation and allied stability         | Favors repair, escort, anti-air coverage, reinforcement, and relief                    |
| Fortification | Values holding exposed economic and territorial assets | Favors layered coverage, durable sites, mines, and defensible geometry                 |
| Greed         | Values growth and future production capacity           | Favors workers, renewable income, expansion, upgrades, and calculated economic risk    |
| Guile         | Values asymmetric pressure and information advantages  | Favors raids, vulnerable targets, feints, opportunistic timing, and earlier withdrawal |

These are tendencies, not exclusive archetypes. A greed-heavy bot still defends
an existential threat. A fortification-heavy bot still attacks when pressure or
opportunity calls for it. An air-light bot still uses aircraft when terrain
makes ground attack impossible. The seed changes decisions among legal options;
it does not determine whether the bot is allowed to understand the game.

## Difficulty is competence, not identity

Difficulty and personality are separate axes. Given the same evidence,
personality changes priorities, composition, and style. Difficulty changes fair
cognitive and execution limits such as attention, reaction time, memory,
estimate accuracy, commitment quality, and coordination.

Every difficulty retains the complete strategic repertoire and obeys the same
costs, prerequisites, queues, movement, combat, vision, and command rules. A
lower difficulty may miss or mishandle an opportunity. It must not be made
easier by forbidding a strategy, shrinking a legal capability, or granting its
opponent hidden advantages.

## Forecasts are not credit

Forecasts may influence the size and timing of a proposal, but commands spend
only scrap actually present in the bank. Completed recurring income may justify
preparing a larger operation whose purchases arrive over time. Unfinished,
deferred, threatened, or merely possible income must be represented with the
appropriate uncertainty and must never become bot-only credit.

Likewise, production forecasts must account for completed producers, their
existing queues, train times, travel time, and a fixed deadline. Adding another
factory can make a larger force attainable. Repeatedly extending the deadline
until every aspiration becomes attainable is not forecasting; it is hoarding.

## Scale without arbitrary caps

Do not impose a controller cap where the player has none. A numerical boundary
is justified when it represents a real decision constraint, such as:

- the value and durability of the objective;
- current and remembered hostile strength;
- protected home defense;
- available scrap and conservative income;
- producer throughput before a fixed deadline;
- builder availability and construction time;
- usable map space, routes, landing sites, or firing geometry;
- diminishing returns or conflict with another valuable investment.

Queue depth, safety floors, phase deadlines, and minimum tactically complete
forces are useful bounds. “Build exactly one Moth,” “never own more than two
Foundries,” or “stop expanding after this personality reaches its quota” are not
useful bounds when no observed opportunity or game rule explains them.

More wealth should permit larger or concurrent investments when worthwhile, but
it should not force waste. A small exposed outpost may not justify an enormous
strike package. A wealthy enemy capital, dense economic cluster, or decisive
opening may.

## Proposals, commitments, and stability

Before commitment, a plan may grow or change as evidence, income, technology,
and capacity change. Existing and queued investments remain accounted for and
should not be discarded simply because a stronger provider became available.

Commitment creates stability. At a domain-specific boundary, freeze the exact
site, builder, force, route, or other resources required for execution. Later
windfalls should not continuously rewrite an attack already under way. New
evidence may still trigger an explicit abort, recovery, or successor plan.

Every commitment needs a completion, cancellation, and recovery path. Memory,
cooldowns, confidence decay, and hysteresis should prevent repeated failed
orders and rapid oscillation without making the bot permanently afraid of a
recovered opportunity.

## Evaluation

Tests should cover the decision boundary that owns a behavior:

- observation and memory tests for what the bot may know;
- pure tests for forecasts, investment cases, and scale monotonicity;
- ownership and accounting tests for live, queued, planned, and reserved work;
- adversarial tests for deadlines, stale evidence, lost prerequisites, and
  conflicting commitments;
- composed scenarios for recognizable multi-step strategies;
- deterministic replays for complete-match inspection.

Useful metrics include resource allocation by domain, planned versus launched
force, idle production capacity, time to impact, rejected commands, stalled
plans, abort reasons, target damage, and whether new observations changed the
decision they should have changed. Metrics can expose contradictions and
pathologies. Only watching and playing representative matches can establish
whether the resulting opponent is understandable, challenging, and fun.

## Architectural direction

The existing controller boundaries remain valuable: fog-honest observation,
timestamped intelligence, persistent domain planners, exact reservations, intent
lowering, ordinary commands, deterministic replay, and the frozen Overseer
evaluation anchor.

Generalization should happen at proven coordination seams: investment cases,
resource claims, production demand, capacity forecasts, and exact ownership.
Target selection, placement geometry, tactical phases, retreat logic, and unit
micro should remain domain-specific where their mechanics differ. The aim is a
coherent strategic organism, not one universal optimizer or a generic planner
that erases the reasons each domain is different.
