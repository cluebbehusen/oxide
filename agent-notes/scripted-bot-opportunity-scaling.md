---
created: 2026-09-01T09:18:44
updated: 2026-09-02T05:58:38
---

# Scripted Bot Opportunity Scaling

## Goal

Make the player-facing bot scale attacks and expansion with deterministic
economic opportunity instead of arbitrary strategic ceilings.

## Decisions

- Fix unfinished-defense threat accounting and missing public-Extractor scouting
  independently before redesigning strategic scaling.
- Use separate small branches and ready-for-review pull requests for the two
  prerequisite bugs.
- Let personality alter investment thresholds, risk tolerance, and composition
  preference rather than permanently forbid useful units or additional
  Foundries.
- Keep legitimate operational bounds such as one unpaid expansion claim, finite
  queues, phase timeouts, and a manifest frozen at commitment.
- Preserve deterministic, fog-honest decisions and require explicit human
  approval before any `SIM_VERSION` bump.
- Approved a same-version player-facing fixture refresh for policy-only bot
  drift; keep `SIM_VERSION` at `0.16.0`.
- Replace the cumulative per-Foundry army gate with candidate-local security:
  preserve one network core, price actionable local threats, and let economic
  value justify preparing missing protection without waiving it.
- Finance connected operations from protected cash plus a bounded
  renewable-income forecast with a non-extending deadline; keep exact membership
  frozen after commitment.
- Landed opportunity-scaled Foundry expansion as the first bounded slice. The
  remaining force-composition and broader allocation work follows the ten-PR
  migration in `scripted-bot-strategic-reset.md`.
- Deferred worker, ordinary-specialist, static-defense, factory, raid,
  team-relief, and upgrade ceilings from the Foundry slice; the strategic-reset
  workstream owns their later migration.

## Findings

- Connected operations hard-code at most three combat members. The strategic
  planner exclusively owns Moth production, so a completed one-Moth siege
  operation leaves repeatable Sentinel and Lancer production as the fallback.
- Island air operations already grow with renewable infrastructure and fighting
  strength, providing a deterministic precedent for resource-driven connected
  operations.
- `PublicMapBriefing` already exposed every authored Extractor frame; the
  pre-fix utility scout ignored those public priors, so restoration and
  expansion never learned about an unseen rich cluster.
- The pre-fix air-defense assessment retained a hostile building's completion
  state but still granted unfinished Flak Turrets full weapon coverage.
- Scout loss policy must retain the role assigned at dispatch time; recomputing
  it from the next current objective can misclassify a completed ground probe or
  a lost dedicated flyer.
- Before the merged expansion change, player-facing greed resolved to a hard
  two-to-four-Foundry ceiling that rejected valuable affordable frontiers.

## Actions

- [x] Land the unfinished-defense anti-air correction with focused regressions.
  - Opened ready PR [#27](https://github.com/cluebbehusen/oxide/pull/27);
    unfinished static AA remains known but contributes no coverage until
    complete, with frozen and player-facing hashes unchanged.
  - Merged into `main`.
- [x] Land public Extractor clusters as fog-honest scouting objectives with
      focused regressions.
  - Opened ready PR [#28](https://github.com/cluebbehusen/oxide/pull/28); rich
    authored clusters now drive fog-honest scouting, completed objectives retire
    cleanly, and temporary route failures preserve bounded air escalation.
  - Review follow-up preserved exact assignment provenance across completed
    probes, retargeting, recalls, and loss; ground probes now route to
    deterministic reachable viewpoints that reveal the full frame.
  - Merged into `main`.
- [x] Audit player-facing strategic count ceilings and classify arbitrary policy
      caps separately from legitimate safety and liveness bounds.
  - Classified Foundry, worker, specialist, defense, factory, raid, relief, and
    upgrade ceilings; distinguished them from demand-scaled Reclaimers,
    Extractors, Airworks, island air operations, lifts, ground muster, and
    reactive mobile AA.
- [x] Design Foundry expansion around marginal economic and strategic return
      rather than a total-count ceiling.
  - Chose external economic return from newly supported Extractors, visible haul
    improvement, and non-self-justifying Foundry income; candidate-local
    security replaces the cumulative army requirement.
- [x] Design connected-operation rosters that grow with spendable wealth,
      renewable income, production throughput, target value, and observed
      defenses before freezing at commitment.
  - Chose an aspirational and achievable roster derived from protected cash,
    renewable income, producer throughput, target value, and observed AA,
    growing only before a fixed deadline.
- [x] Agree on the detailed cap-replacement plan before implementation.
  - Approved two focused PRs, a redesigned expansion-security gate, and bounded
    forecast financing.
- [x] Implement and evaluate opportunity-scaled Foundry expansion across
      representative maps and economic conditions.
  - Replaced the player-facing Foundry count ceiling with exact-site economic
    return, candidate-local security, one unpaid claim, and an escrow that
    prevents later voluntary spending from consuming the commitment.
  - Eliminated the initial large-map route-planning regression through bounded
    exact caches and shared component floods; focused accounting, safety,
    reranking, builder, and escrow coverage passed.
  - Representative Skirmish, Severance, and The Scattering runs expanded without
    stalled or rejected construction; rich Scattering seats reached six
    Foundries without a controller count ceiling.
- [x] Transfer connected-operation scaling and broader cap removal to the
      strategic-reset workstream.
  - Preserved the approved opportunity-scaled force-package direction while
    moving its implementation, production integration, cross-domain allocation,
    and evaluation into `scripted-bot-strategic-reset.md` so only one note owns
    the remaining work.

## Open Questions
