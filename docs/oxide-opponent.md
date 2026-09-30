# oxide-opponent

`oxide-opponent` is a reactive, best-effort opponent controller alongside
`oxide-bot`. This page is its normative specification: what it must do, what it
must keep exact, and what it may not do. [Bot strategy](bot-strategy.md),
[bot architecture](bot-architecture.md) and `docs/bot/` describe `oxide-bot` and
do not apply to this crate. The
[oxide-opponent skill](../.agents/skills/oxide-opponent/SKILL.md) holds the
working rules.

It plays under the same fair-opponent and determinism contracts as every
controller (see `AGENTS.md`): fog-honest information, ordinary costs and rules,
commands as its only effect, and no floating point.

## Principles

- **Reactive.** It responds to what it has seen: enemy air, defenses, pressure
  on its assets, relative wealth, and failed orders.
- **Best-effort.** It scores what it wants, issues ordinary commands, observes
  what happens and recovers. It does not prove that a plan is feasible before
  acting.
- **Legible.** A player should be able to read its reactions: "I built air, so
  it built Flak."

## What stays exact

The bot always knows, exactly:

- scrap on hand and what the current decision has committed;
- which units and buildings each mission owns;
- which workers are assigned to construction;
- which units are queued, which count toward composition;
- what each transport carries.

An emitted purchase is an attempt until a later observation confirms it; the
decision reserves its cost meanwhile. Fog honesty and recovery are exact too.

## What is approximate

Combat predictions, launch timing, danger, own income, enemy economy, and
whether a movement will succeed are estimates. The bot acts on them, accepts
occasional failed movement and missed opportunities, and recovers. It must not
wait for certainty: requiring a favorable estimate, a large enough force, a safe
route and reassuring reconnaissance all at once produces hesitation.

## Decisions

On each decision tick the bot updates memory from its fogged observation and its
own events, estimates the situation, updates its needs and investment list, then
selects actions in precedence order:

1. emergency defense and recovery;
2. the saving target, when it is affordable and can be placed;
3. workers, to keep harvesting saturated;
4. missions, advancing their phases and recruiting as needed;
5. production toward composition needs, from scrap not protected for saving.

Spending and ownership are coordinated through one running total that lives
inside the decision and never outlives it as a type. Difficulty limits unit
orders per decision; purchases do not count against that allowance. The
allowance is consumed as actions are selected, so only selected actions affect
the running total and mission state.

## Investments and saving

- Each decision scores concrete investments: production capacity, tech
  buildings, an expansion Foundry at a specific site, a defense at a specific
  spot, upgrades. Needs, capability needs, personality and situation set the
  scores.
- The top investment is funded through its next purchasable step. The bot keeps
  one optional saving target with its price, reason and cancel conditions.
- Income is not observed directly. The bot estimates it from the change in bank
  since the previous decision plus its own spending in between.
- While saving, a share of income is protected for the target, capped by the
  target's price and the real bank. Other purchases spend only the unprotected
  part. Stance, personality and threat set the share.
- An affordable target is bought ahead of everything except emergencies. Targets
  switch only when another scores clearly higher or the reason disappears;
  protected scrap carries over. Emergencies may spend protected scrap. A
  rejected, cancelled or refunded purchase keeps the target and recomputes the
  protected amount from the bank.

## Missions and production

- Global production buys toward composition needs and the funded investment. No
  mission owns a future production contract.
- Counts come from need. What the bot owns, trains, builds or sends follows a
  need it can state from what it knows; a constant may bound computation or
  model a difficulty, stance or personality limit, never what the bot owns or
  sends.
- Missions recruit from available units in a fixed order (defend, lift, attack,
  raid), and each takes only the roles it uses: attack takes line, siege and
  anti-air units, with a Tender and Sappers in support; raid takes Scuttlers,
  Sappers, or ground-attack aircraft too few for a strike; lift takes carriers
  and a payload.
- A mission takes only the force it needs, keeps it while its purpose holds, and
  releases it when the purpose disappears or recovery finishes. At a safe
  transition it can yield suitable units to an emergency; loaded passengers are
  released only after landing.
- Missions have phases (gather, travel, engage, withdraw, recover; load, fly,
  land, fight) and hysteresis, so repeated scoring does not turn each decision
  into a new plan.
- Capability needs, such as "this objective requires lift" or "enemy air
  requires anti-air", raise production and investment priorities. They reserve
  nothing and promise nothing to a mission.

## Persistent state

Checkpoints preserve all non-derivable state that affects future decisions. No
other non-derivable state persists between decisions:

- memory of enemy units and buildings, and of failed objectives;
- missions and their phases;
- the investment list and saving target;
- the previous bank and spending used to estimate income;
- reaction and hesitation timers, and cooldowns;
- the position of any random stream;
- the pending own-event buffer.

Immutable scenario-derived data, such as the map model, rebuilds on load. Any
retained derived data needs an explicit rebuild or invalidation rule. A new
checkpoint field needs a design review.

## Own events

The host session keeps each seat's own `OrderStalled` and `CommandRejected`
events in a bounded, ordered buffer and passes it with the observation; when the
buffer is full, the oldest event is dropped. The buffer is saved with
checkpoints. A background decision reads it without draining it; events are
consumed when that decision's result is installed. A rejection that cannot be
tied unambiguously to one command triggers a check of the observed outcome; it
never marks a particular purchase as failed.

## Map knowledge

The simulation paths units itself, so the bot needs map knowledge only for
reachability, distance, placement and danger. It builds a small static model
once per match (ground-connected components, a region graph, distance fields
from home and enemy starts, candidate sites) and shares it between clones.
Danger is coarse known weapon coverage. Placement uses a fog-honest check over
the observation. A stall or rejection triggers a re-plan instead of the order
being proven in advance.

## Computation

- **Allowed:** bounded scoring of a small candidate set, flood fills over fixed
  grids, single-command legality and connectivity checks, and small bounded
  route searches. All work has finite inputs and a bounded number of visits or
  expansions.
- **Not allowed:** speculative search over action sequences, multi-step plan
  verification, joint future producer schedules, reproducing the simulation's
  command processing to pre-verify orders, and planning state that spans
  decisions.

## Reactivity

Each behavior is proven by a staged scenario test.

| Situation                            | Required response                                                                    |
| ------------------------------------ | ------------------------------------------------------------------------------------ |
| Enemy air seen                       | Anti-air proportional to seen enemy air                                              |
| Enemy Airworks seen                  | Raise the anti-air need before enemy air arrives                                     |
| Ground threat at base or expansion   | Defend                                                                               |
| Air raid on base or expansion        | Defend with anti-air or fighters                                                     |
| Richer economy than the enemy        | Build toward larger attacks, ground and air                                          |
| Known defenses at target or on route | Bring siege, or pick another target                                                  |
| Ground-severed enemy                 | Air operations or lift                                                               |
| Local opportunity                    | Attack when the known local defense is beatable now, above a stance-bounded minimum  |
| Losing a fight                       | Withdraw, with a threshold shaped by personality and difficulty                      |
| Won a defense with army left         | Counterattack if the attack check passes                                             |
| Enemy artillery shelling assets      | Prioritize and reach the artillery; raise the need for mobile and artillery counters |
| Assets under indirect fire           | Target visible likely spotters                                                       |
| Enemy unit mix                       | Choose units by coarse suitability against seen enemies                              |
| Expensive investment wanted          | Save for it while keeping an army share                                              |
| Several enemies                      | Choose a target with hysteresis                                                      |

It also covers scouting with a re-scout after unexplained losses, Scuttler
raids, harvest-line harassment by air, team relief, escorts, repair, harvester
evacuation, emergency and voluntary static defense, Extractor restoration,
expansion timing, tech prerequisites, memory of failed objectives, a response to
stalled production, focus fire at Veteran and Prime, and pulling wounded units
back between fights. It does no other per-unit micro. It builds every building
kind, reaches every upgrade tier, and trains every unit its faction fields.

## Difficulty, stance and personality

- **Personality** resolves deterministically from the seat's personality seed,
  bounded by stance and a fixed trait budget. Its traits weight every score and
  never grant or remove a capability.
- **Stance** bounds posture: minimum home defense, minimum attack size, and how
  early and how large attacks get. Offensive missions leave the home defense
  behind while an enemy could reach home; defense takes every unit.
- **Difficulty** sets cognitive and execution limits: reaction delay,
  hesitation, unit orders per decision, memory decay, deterministic noise in
  estimates, and decision interval. Lower rungs make understandable mistakes,
  and higher rungs beat lower ones.
