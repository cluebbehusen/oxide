# Tactical missions and execution

[Architecture and ownership map](../bot-architecture.md).

## Controller-local battlefield loop

Each army stores its accepted mission alongside its tactical state. Policy
orients that combined view once. Executive keeps outcome journals separately:
pending evidence can outlive the body's removal or reorganization. Executive
drains reports in army-ID order before retiring missing bodies' journals; Brain
orients and learns from those reports before scoring its next decision.

Ground mission planning prepares each available army's center, ground-capable
count, and marching strength once per decision. Unchanged armies are borrowed;
only staging armies with newly unavailable members need an owned filtered copy.
Retained servicing, defense assignment, and pressure/reserve selection share one
decision-local set of unit claims, attention usage, and lazy route preparation.
They execute in that order because each consumes capacity available to the next.

Playable army contact includes observed completed defenses with compatible
weapon range and known fire geometry. Local strength counts nearby participants
and the defenses actually covering them. An army returns after a newly observed
casualty if neither the previous nor current observation showed contact. It
records an inconclusive episode without inventing a hidden attacker. Visible
objective completion takes precedence; earlier combat casualties do not trigger
a later return. Withdrawal from static fire also uses ordinary movement to avoid
reacquiring the position. Pressure admission counts known gun coverage along a
projected approach as well as defenders near the objective. Against a building,
an escorted siege body seeks a reachable position inside its guns' range and its
screen's sight. It prefers positions outside known defensive fire; if the
defender matches its range, an already-admitted assault accepts exposure instead
of waiting forever for range superiority. Its faster screen advances at most
three route steps ahead of the rearmost gun, then waits for it before
approaching the shared firing position. Artillery with a visible target in range
remains engaged rather than being discarded as a stalled march.

Player-facing maintenance advances tactical armies first. The decision then
observes battlefield evidence and work outcomes once, including on the early
economy-recovery path, before preparing new investment alternatives. Spatial
contact bins distinguish physical and target domains. Movement history stores
two actual observations and their ticks, never an extrapolated position. Radar
contributes unresolved regions rather than identified units. Reachable local
service is credited once when describing uncovered asset pressure.

Utility work journals own dispatched harvest probes, construction attempts,
foundation watches and bounded failure exclusions. After lowering, the
controller records emitted commands in output order, converting their positions
back into the policy's orientation. An immediate retask clears the superseded
harvest probe and closes affected attempts; queued movement preserves the
current assignment. Observation advances this work once per tick before
allocation checkpoints. Residual utility planning does not repeat the audit or
maintain a separate pending-site lifecycle. Focused policy tests use this
maintained preparation and utility context inside the bot crate; controller
integration tests use `Brain` or `SeatBot`.

The immutable assessment feeds reconnaissance, protective support, Array
coverage, and general ground-mission selection. Fresh deployments are finalized
after allocation, against its exact reservations. Army responsibilities belong
to the Executive's existing `ArmyId`; they do not create another unit lease in
the allocator. Exact formation and reorganization validate the complete change
before modifying either body. Staging splits retain coherent groups and home
strength. Engaged or withdrawing bodies are not split. Ordinary movement handles
recovery so tactical reacquisition cannot restart an abandoned chase; observed
return explicitly reopens reserve reinforcement. Tactical emergency withdrawal
has precedence over a mission directive and replaces the abandoned mission with
recovery after recording its outcome. An accepted defense assignment dispatches
its exact body even when proximity has already marked it engaged. Lowering
receipts expose actual acceptance and refusal boundaries independently of
command counts.

Pressure objectives bind the observed owner, kind, and complete footprint, plus
a live id only when current sight supplied one. Remembered placeholders are not
entity identities: reacquiring the same site preserves its mission and fixed
deadline, while another remembered site cannot keep it alive. Objective anchors
use footprint orientation independently of movement goals. Outcome watches
resolve current identity at that exact site before measuring observed damage.

Operation and work owners submit bounded episode reports to `experience`.
Ground, air, transport, raid, relief, reconnaissance, repair, harvest, and
construction reporting use observed progress and owner-only presence. Carried
units remain present but unavailable. Foundation observation continues after the
builder leaves; a delivery watch continues without ownership of its landed
troops. Shared coordination credits prevent these components from teaching the
same outcome repeatedly. Stronger assault evidence may replace preliminary
delivery credit. Ambiguous attribution cannot earn broad doctrine credit.
Inconclusive lost-contact or deadline reports with known own casualties retain
half-strength contextual evidence and can request approach reconnaissance. They
remain inconclusive about the objective and never contribute to doctrine. Frozen
objective owner, kind, and footprint anchor link failed approaches to remembered
buildings without relying on their placeholder ids. Ground reports entering
policy memory orient both their context point and frozen objective footprint, so
later observation matching uses one coordinate frame. Defensive service uses a
provider's movement domain for routing, including parked aircraft; target
exposure still uses its current body domain.

Experience subjects distinguish building and unit identities, construction and
production kinds, upgrades, harvest, and reconnaissance. Construction reports
and allocation queries share the same context constructor. Related work shares
broad doctrine preferences deliberately; matching numeric IDs or enum ordinals
do not transfer contextual credit. Trace subjects serialize their variant and
value instead of an untyped integer.

Contextual return and corroborated doctrine preferences decay toward neutral.
Each retained credit owns its strongest contextual evidence and, independently,
its strongest qualified doctrine evidence, including report identity,
confidence, completion time and score. Weaker follow-ups cannot replace either
winner after the recent report history evicts it. Evidence expires at its own
completion time; new evidence for another credit cannot renew it. Replacing
shared credit moves only that credit. Storage retains at most 64 credits per
context, 128 contexts, and 64 recent episode reports, with canonical
oldest-first eviction. Credit eviction uses the newer of its retained contextual
and doctrine evidence timestamps, breaking ties by credit identity. Recent
reports support diagnostics and approach reconnaissance; both preference scores
come from retained credit evidence. Context scores fold in canonical completion
order with saturation so counterevidence can break an entrenched preference.
Experience refreshes its six doctrine scores after observation or a report
changes retained evidence. Proposal queries read those prepared scores rather
than rescanning every credit; contextual lookup inspects at most one bounded
context's contributions. These preferences alter candidate ranking and effective
allocation return without rewriting raw consequence, urgency, confidence, or
safety. Retry records for dispatched harvest and construction attempts expire
and require fresh legal preparation. Current footprint occupation invalidates a
construction attempt without a route penalty; remembered buildings alone cannot
establish that occupation. Work observation also indexes active builders'
occupied tiles once per decision. Fresh blocking foundations cannot displace
those workers from their current work tiles; movement and completion release
this protection without changing ordinary terrain routing. Contested-harvest
quarantine retains its separate complete-sweep and safe-return requirements.
These components are reconstructed by replaying the observed command prefix in
the existing replay loader. Internal session checkpoints preserve them directly,
together with unfinished planning and its remaining work allowance. Neither path
puts controller memory in authoritative `State`. Controller checkpoint
restoration validates map, configuration, planning storage, and time boundaries
before exposing a seat; navigation query caches rebuild without changing
decision work allowances.

An unpaid Foundry's recovery interval spans both funding and execution blockage.
Restored funding permits another readiness check; only a ready builder and site
clear the interval. Continuous execution blockage therefore releases the unpaid
claim after the same bounded recovery period as continuous funding failure.
