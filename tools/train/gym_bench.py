"""Measure where training rollout time actually goes.

Runs episodes through the same Worker/driver loop the league uses and
prints the wall-time split: driver-side simulation (state ticking plus
the in-process Overseer and gym executives), driver reply encoding,
pipe writes, client wire wait, client JSON parse and frame build, and
the policy itself. The residual is Python loop overhead.

Two policies are available: ``--ckpt`` runs a real torch checkpoint
greedily (the league's inference cost included); omission steps an
instant first-legal-action policy, which isolates the environment and
wire from inference entirely.

Usage, from tools/train:

  uv run gym_bench.py --episodes 4
  uv run gym_bench.py --episodes 4 --ckpt runs/r17-distilled.pt
"""

import argparse
import os
import time
from typing import TYPE_CHECKING

from oxide_gym import ACTION_HEADS, CADENCE, ActionPlan, SeatView, Worker

if TYPE_CHECKING:
    from collections.abc import Callable


def instant_policy(view: SeatView) -> ActionPlan:
    return (
        next(a for a in ACTION_HEADS[0] if view.mask[a]),
        next(a for a in ACTION_HEADS[1] if view.mask[a]),
        next(a for a in ACTION_HEADS[2] if view.mask[a]),
        next(a for a in ACTION_HEADS[3] if view.mask[a]),
    )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--episodes", type=int, default=4)
    ap.add_argument("--scenario", default=None, help="scenario path (default skirmish)")
    ap.add_argument("--max-ticks", type=int, default=20_000)
    ap.add_argument("--cadence", type=int, default=CADENCE)
    ap.add_argument("--seed-base", type=int, default=9_000)
    ap.add_argument(
        "--ckpt", default=None, help="torch checkpoint (default instant policy)"
    )
    ap.add_argument(
        "--driver",
        default=os.environ.get("OXIDE_DRIVER_BIN", "../../target/release/oxide-driver"),
    )
    args = ap.parse_args()

    act: Callable[[SeatView], ActionPlan] = instant_policy
    if args.ckpt:
        # Optional heavyweight dependency, loaded only for the ckpt arm.
        import torch  # noqa: PLC0415, I001
        from models import factorized_greedy, load_policy  # noqa: PLC0415

        net, _ = load_policy(args.ckpt)
        net.eval()

        def checkpoint_policy(view: SeatView) -> ActionPlan:
            with torch.no_grad():
                logits, _ = net(
                    torch.as_tensor(view.obs[None]), torch.as_tensor(view.mask[None])
                )
            p = factorized_greedy(logits)[0].cpu()
            return (int(p[0]), int(p[1]), int(p[2]), int(p[3]))

        act = checkpoint_policy
    worker = Worker(args.driver)
    policy_s = 0.0
    decisions = 0
    started = time.perf_counter()
    try:
        catalog = worker.profile_catalog
        for episode in range(args.episodes):
            profile = catalog.profiles[episode % len(catalog.profiles)]
            condition = catalog.condition(
                profile.style, profile.variant, catalog.default_role, "ferrous"
            )
            frame = worker.reset(
                seed=args.seed_base + episode,
                control=(0,),
                max_ticks=args.max_ticks,
                cadence=args.cadence,
                scenario=args.scenario,
                conditions={0: condition},
            )
            while not frame.done:
                view = frame.seats.get(0)
                if view is None:
                    frame = worker.step({})
                    continue
                t0 = time.perf_counter()
                plan = act(view)
                policy_s += time.perf_counter() - t0
                decisions += 1
                frame = worker.step({0: plan})
        wall = time.perf_counter() - started
        stats = worker.timing_stats()
    finally:
        worker.close()

    ticks = stats["ticks"]
    rows = [
        (
            "driver sim: state ticking",
            (stats["sim_us"] - stats.get("opponent_us", 0)) / 1e6,
        ),
        ("driver sim: overseer opponents", stats.get("opponent_us", 0) / 1e6),
        ("driver reply build+encode", stats["reply_us"] / 1e6),
        ("driver pipe write", stats["write_us"] / 1e6),
        ("driver scenario resets", stats["reset_us"] / 1e6),
        (
            "client wire wait minus driver busy",
            max(
                0.0,
                (
                    stats["client_wait_us"]
                    - stats["sim_us"]
                    - stats["reply_us"]
                    - stats["write_us"]
                    - stats["reset_us"]
                )
                / 1e6,
            ),
        ),
        ("client parse + frame build", stats["client_parse_us"] / 1e6),
        ("client send", stats["client_send_us"] / 1e6),
        ("policy", policy_s),
    ]
    accounted = sum(seconds for _, seconds in rows)
    rows.append(("python loop residual", max(0.0, wall - accounted)))
    label = args.ckpt or "instant"
    print(f"\nGYM ROLLOUT SPLIT  ·  {args.episodes} episodes  ·  policy {label}")
    print(
        f"wall {wall:.2f}s  ·  {ticks} ticks ({ticks / wall:,.0f}/s)  ·  "
        f"{decisions} decisions ({decisions / wall:,.0f}/s)  ·  "
        f"{stats['client_bytes_received'] / 1e6:.1f} MB received"
    )
    for name, seconds in rows:
        print(f"  {name:<38} {seconds:>8.2f}s  {100 * seconds / wall:>5.1f}%")


if __name__ == "__main__":
    main()
