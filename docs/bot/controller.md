# Controller and profiles

[Architecture and ownership map](../bot-architecture.md).

## Crate boundary

`oxide-bot` depends on `oxide-sim`, never the reverse. Simulation owns the
observation schema and authoritative fog filtering; bot owns derived caches,
profile resolution, memory and decisions. Shared command geometry stays with the
rules so prediction and execution use the same tie-breaks.

`Brain` accepts observations. `SeatBot` is the state-aware host adapter, gating
finished matches, cadence and resignation before invoking the decision core. The
existing host worker pool captures each due seat's observation on its worker and
collects commands in canonical seat order. No observation prepass serializes
that work, and policy does not create nested workers. A bot-only source edit
does not invalidate the simulation crate's compiled code.

## Controller and profiles

Bots live outside `State::tick`. A bot reads a state-derived observation and
emits ordinary `PlayerCommand` values, which the shell or runner records before
the simulation sees them. A configured seat carries one strict `BotConfig` with
a difficulty, stance, and personality seed. `seat_bots` passes that exact setup
and one shared immutable scenario briefing to the fog-honest `Brain::scripted`
controller. Player-facing decisions stop when the own seat resigns or has no
remaining Foundry, even while teammates keep the match alive; remnant units
continue their ordinary simulation programs without new bot commands.
Scenario-origin replay continuation rebuilds the briefing and observes the
recorded command history; player checkpoints restore retained memory directly.
Neither path consults ambient input.

Profile resolution turns the seed into six bounded preferences: air, siege,
support, fortification, greed, and guile. Stance bounds their strategic posture;
difficulty changes fair cognitive, execution, and macro-competence limits such
as reaction, attention, memory, estimate accuracy, commitment timing, and the
minimum ordinary opening force protected from voluntary spending. Scrapheap also
uses a reduced decision cadence; Standard, Veteran, and Prime share one
competent cadence. Neither mechanism grants information, resources,
capabilities, or stronger units. Personality also does not vary private
competence: each difficulty uses one fixed conservative strength-estimation
error. The traits leave visible signatures rather than unlocking private
strategies: air changes ordinary and island strike composition and timing; siege
changes artillery volume and preference; support changes support-unit, flak, and
allied relief investment; fortification changes cross-domain defense priority
and otherwise competitive defensive roles; greed changes worker targets and
renewable-expansion payback appetite; and guile changes raid size, timing,
withdrawal, and some mine or airborne-screen emphasis. Every adaptive identity
may propose every defensive role its ordinary prerequisites permit. Exposed
value, credible approach evidence, existing coverage, and diminishing return
bound investment instead of personality-derived count ceilings. Before the
protected core exists, a current visible threat may justify one matching
emergency Turret, while emergency Flak requires a current visible aircraft
capable of attacking ground. A pure air-to-air flyer cannot unlock that
exception. A player-facing controller requires this actionable air evidence
before investing in emergency flak, so an anonymous radar blip cannot turn a
small seeded preference into an opening economy cliff.

Difficulty schedules are structurally monotone. Scrapheap thinks every 24 ticks;
Standard, Veteran, and Prime share a 12-tick cadence so controller APM does not
invert the difficulty ladder. Every decision tick available to a lower rung
remains available to the next higher rung, while reaction and commitment delays
shrink and attention and memory never shrink. Private uncertainty is fixed per
rung and conservative: a lower rung never estimates its own force as stronger,
or a hostile force as weaker, than a higher rung using the same evidence. Their
stance- and personality-independent opening floors are respectively four, five,
six, and eight Sentinel-equivalents, so the protected macro commitment is
monotone as well. Veteran and Prime coordinate engaged army fire. Prime also
uses the ordinary focus-fire command to direct an overlapping static-defense
line at one current visible ground threat; the simulation retains ordinary
acquisition whenever that preference is blocked or out of range. Veteran and
Prime share the same optional-operation attention ceiling, so Prime does not
split off a raid while air and lift work already run together.

Every ordinary difficulty cadence divides a shared 24-tick strategic admission
interval. New air, lift, and raid operations, a remembered air objective's
promotion to a current assault, and the start of a team-relief pressure watch
use those common boundaries. This lets every rung freeze the same world snapshot
before its own reaction and commitment delays take effect; private controller
cadence never grants an earlier strategic observation boundary. A team-relief
credibility watch samples current pressure at such a boundary. It prepares an
exact route-capable group without starting or owning a deployment. Only a
successful Support allocation launches that frozen group; an active relief
advances once per decision even when fresh admission is closed. Useful current
pressure and protected home strength determine membership, with a two-member
tactical minimum and no personality eligibility or group-size cap.

## Configuration and replay

The current wire format deliberately has one maintained controller, `scripted`;
only difficulty, stance, and personality seed are stored, not the resolved
traits or planner memory. Replays record the commands a bot emitted, so
read-only playback does not rerun that controller. Replay compatibility remains
governed by the simulation version rather than by retaining obsolete bot
implementations. Authored scenarios and current-version replays accept only the
current shape. The Oxide replay loader recognizes known retired
bot-configuration shapes only inside a replay stamped with another simulation
version, normalizing that setup metadata so deliberate archaeology can reach the
version check. Serialization emits only the current shape.

## Controller checkpoints

`SeatBot::checkpoint` borrows the seat and stores a canonical CBOR payload.
Restore checks the seat, map dimensions and orientation against the bound
session. Profile, authored/oriented briefings and policy dials rebuild from the
scenario. Observational query caches can be recomputed from their current
inputs. Decision memory, dynamic knowledge, work cursors and unfinished searches
survive. Completed approach fields persist their recipes and use ages; restore
reconstructs their answers from saved knowledge before installing the
controller, without spending the live planning allowance or changing readiness
or retention. Expanded storage is bounded before rebuilding. See
[navigation](navigation.md) for work lifetimes and invalidation.

Owner validators reject memory that could panic or cause unbounded work, not
forged history that merely changes play. They do not duplicate tuning formulas
or authenticate controller history. The controller format remains at revision 1
while player saves are unpublished; no legacy layout migration is provided.
Controller memory is separate from authoritative `State` and its hash. The
[shell persistence contract](../shell-architecture.md#persistence-and-replay)
owns session binding, file format and background-worker behavior.

## Diagnostic traces

Player-facing bot decision traces are output only. An opt-in `Brain::act_traced`
call reports only facts already owned by the fog-honest coordinator while
returning the same ordinary commands as `Brain::act`. The trace recorder is
local to that call; traces are not controller memory, authoritative state,
replay input, or replay metadata. Ticks on which no player-facing decision
occurs produce no trace. Trace schema version 13 reports current scrap
separately from a bounded forecast based only on completed income sources,
together with current builder and producer capacity. Proposal and allocation
evidence records the coordinator's actual inputs and verdicts, including
economic action and defensive proposal identities, exact building claims, exact
repair ownership, refit income losses, and arbitrary-size layout conflicts,
rather than reconstructing decisions after the fact. Battlefield evidence,
Executive mission ownership, bounded episode reports, decayed preferences, and
effective allocation return are separate trace fields. Raw proposal consequence,
urgency, confidence, and safety are not rewritten to express historical
preferences.
