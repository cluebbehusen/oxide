"""Doctrine demonstrations: the policy teaches itself its missing units.

Scripted-teacher cloning (bc.py) moves the trunk toward the teachers'
whole playstyle — measured tonight, that traded away the candidate's
learned rush defense without buying voluntary usage. This variant
keeps the demonstrations inside the policy's own distribution: the
candidate plays its normal greedy game on the duel slate while a
doctrine overrides one head toward a target kind whenever the target
is mask-legal and the seat holds fewer than its quota, chaining
through a missing producer exactly like the driver's viability probe.
The corpus is ordinary candidate play plus the one behavioral delta,
so cloning it back is replay with a lesson, not a personality
transplant.

Usage (from tools/train/):
    uv run doctrine_bc.py --resume runs/night2/es1-exact.pt \
        --episodes 48 --out runs/night2/doctrine-bc.pt
"""

from __future__ import annotations

import argparse
import pathlib

import numpy as np
import torch

from bc import (
    BUILD_AIRWORKS,
    BUILD_BASTION,
    BUILD_FLAK,
    BUILD_FOUNDRY,
    FACTION_PAIRS,
    TRAIN_AIR_AA,
    TRAIN_INTERCEPTOR,
    TRAIN_SAPPER,
    TRAIN_SCOUT_FLYER,
    TRAIN_SCUTTLER,
    TRAIN_TENDER,
    TRAIN_TRANSPORT,
    TRAIN_WING,
    F,
    duel_scenarios,
    local_action_targets,
)
from lineage import build_lineage, content_digest, input_identity
from models import factorized_greedy, load_policy, save_policy
from oxide_gym import (
    ACTION_HEADS,
    CONSTRUCTION_HEAD,
    GYM_VERSION,
    PRODUCTION_HEAD,
    ActionPlan,
    FactionName,
    Worker,
)

BUILD_FAB = 9
BUILD_ARRAY = 13
BUILD_RECLAIMER = 14
BUILD_CRUCIBLE = 37
FORM_ARMY = 17
PUSH = 18
SCOUT = 20
AIRLIFT = 41

# Each lesson: the forced action, the raw-feature count that meters its
# quota, the quota itself, and the producer chain (build action + its
# count feature) walked root-first while the target's mask is closed.
LESSONS: dict[str, tuple[int, str, int, list[tuple[int, str]]]] = {
    "skyhook": (
        TRAIN_TRANSPORT,
        "my_transports",
        2,
        [(BUILD_AIRWORKS, "airworks_built")],
    ),
    "interceptor": (
        TRAIN_INTERCEPTOR,
        "my_interceptors",
        2,
        [(BUILD_AIRWORKS, "airworks_built")],
    ),
    "scout-flyer": (
        TRAIN_SCOUT_FLYER,
        "my_scout_flyers",
        1,
        [(BUILD_AIRWORKS, "airworks_built")],
    ),
    "air-ground": (TRAIN_WING, "my_airground", 2, [(BUILD_AIRWORKS, "airworks_built")]),
    "air-air": (TRAIN_AIR_AA, "my_airair", 2, [(BUILD_AIRWORKS, "airworks_built")]),
    "tender": (TRAIN_TENDER, "my_tenders", 1, [(BUILD_FAB, "fab_built")]),
    "sapper": (TRAIN_SAPPER, "my_sappers", 2, [(BUILD_FAB, "fab_built")]),
    "scuttler": (TRAIN_SCUTTLER, "my_scuttlers", 2, []),
    "flak": (BUILD_FLAK, "my_flak_built", 2, []),
    "bastion": (BUILD_BASTION, "my_bastions_built", 2, []),
    "airworks": (BUILD_AIRWORKS, "airworks_built", 1, []),
    "foundry": (BUILD_FOUNDRY, "my_foundries_built", 2, []),
    # The Deep Array sits behind the forge gate, so its lesson must be
    # able to raise the Crucible (and the Crucible's Fabricator) first.
    "array": (
        BUILD_ARRAY,
        "my_arrays_built",
        1,
        [(BUILD_CRUCIBLE, "crucible_built"), (BUILD_FAB, "fab_built")],
    ),
    "reclaimer": (BUILD_RECLAIMER, "my_reclaimers_built", 1, []),
    "crucible": (BUILD_CRUCIBLE, "crucible_built", 1, [(BUILD_FAB, "fab_built")]),
}

# The hunt is not a quota lesson: it drills the finishing loop the
# stalled endgames never ran. While no enemy Foundry is known, scouts
# fly; once one is known, idle fighters stage and a big enough army
# commits. Handled specially by apply_doctrine.
HUNT_LESSON = "hunt"

# The ferry drills the island assault the ground lessons cannot reach:
# transports first, then the Airlift shuttle (an empty sling gathers
# idle fighters, a loaded one drops at the known enemy site). FormArmy
# is deliberately absent — enlisted fighters are not liftable.
FERRY_LESSON = "ferry"


# Which canonical profile teaches which lesson. A lesson taught under a
# zero-facet condition has no input signature — the net averages the
# contradictory labels into compulsions. Keyed to the facet knobs, the
# taught preference lands where the personality system already points:
# air units under the air-facet profiles, fortification under the
# turtle variants, expansion under the economy lean, sappers under
# siege, scuttlers under the swarm's commitment.
PROFILE_LESSONS: list[tuple[str, int, str]] = [
    ("balanced", 1, "skyhook"),
    ("aggressive", 1, "skyhook"),
    ("balanced", 1, "airworks"),
    ("balanced", 1, "interceptor"),
    ("aggressive", 1, "air-ground"),
    ("balanced", 1, "air-air"),
    ("balanced", 1, "scout-flyer"),
    ("turtle", 0, "bastion"),
    ("turtle", 2, "bastion"),
    ("turtle", 0, "flak"),
    ("turtle", 0, "tender"),
    ("turtle", 1, "tender"),
    ("turtle", 1, "foundry"),
    ("balanced", 0, "foundry"),
    ("balanced", 2, "sapper"),
    ("aggressive", 2, "sapper"),
    ("aggressive", 0, "scuttler"),
    ("balanced", 2, "crucible"),
    ("aggressive", 2, "crucible"),
    ("turtle", 1, "array"),
    ("balanced", 0, "array"),
    ("turtle", 0, "reclaimer"),
    ("turtle", 2, "reclaimer"),
    ("balanced", 0, "reclaimer"),
    ("aggressive", 0, "hunt"),
    ("aggressive", 1, "hunt"),
    # Ferry stays off variant 1: the contact-cohort style gate reads the
    # flagship variants' identity directly, and a flagship taught to
    # trade fighters for transports fails its force signature.
    ("aggressive", 0, "ferry"),
    ("aggressive", 2, "ferry"),
    ("balanced", 0, "ferry"),
    ("balanced", 2, "ferry"),
    ("turtle", 2, "ferry"),
]

FACTION_NAME: dict[str, FactionName] = {"f": "ferrous", "c": "cupric"}

PRODUCTION_SET = frozenset(PRODUCTION_HEAD)
CONSTRUCTION_SET = frozenset(CONSTRUCTION_HEAD)


def apply_doctrine(
    plan: ActionPlan,
    lesson: str,
    raw: list[int],
    mask: np.ndarray,
) -> tuple[ActionPlan, bool]:
    """Returns the plan with the lesson's override applied, and whether
    anything changed. Legality always comes from the shared mask."""
    if lesson == HUNT_LESSON:
        return apply_hunt(plan, raw, mask)
    if lesson == FERRY_LESSON:
        return apply_ferry(plan, raw, mask)
    action, counter, quota, chain = LESSONS[lesson]
    if raw[F[counter]] >= quota:
        return plan, False
    production, construction, upgrade, operation = plan
    if mask[action]:
        if action in PRODUCTION_SET and production != action:
            return (action, construction, upgrade, operation), True
        if action in CONSTRUCTION_SET and construction != action:
            return (production, action, upgrade, operation), True
        return plan, False
    for link, link_counter in chain:
        if raw[F[link_counter]] == 0 and mask[link]:
            if construction != link:
                return (production, link, upgrade, operation), True
            return plan, False
    return plan, False


def apply_hunt(
    plan: ActionPlan,
    raw: list[int],
    mask: np.ndarray,
) -> tuple[ActionPlan, bool]:
    """Find, stage, commit: the close-out loop as a doctrine."""
    production, construction, upgrade, operation = plan
    if raw[F["enemy_foundry_known"]] == 0:
        if mask[SCOUT] and operation != SCOUT:
            return (production, construction, upgrade, SCOUT), True
        return plan, False
    if raw[F["staging_army_size"]] >= 6 and mask[PUSH]:
        if operation != PUSH:
            return (production, construction, upgrade, PUSH), True
        return plan, False
    if mask[FORM_ARMY] and operation != FORM_ARMY:
        return (production, construction, upgrade, FORM_ARMY), True
    return plan, False


def apply_ferry(
    plan: ActionPlan,
    raw: list[int],
    mask: np.ndarray,
) -> tuple[ActionPlan, bool]:
    """Transports, then the shuttle: gather, cross, drop, repeat."""
    production, construction, upgrade, operation = plan
    if raw[F["my_transports"]] < 2:
        if mask[TRAIN_TRANSPORT] and production != TRAIN_TRANSPORT:
            return (TRAIN_TRANSPORT, construction, upgrade, operation), True
        if (
            raw[F["airworks_built"]] == 0
            and mask[BUILD_AIRWORKS]
            and construction != BUILD_AIRWORKS
        ):
            return (production, BUILD_AIRWORKS, upgrade, operation), True
    if mask[AIRLIFT] and operation != AIRLIFT:
        return (production, construction, upgrade, AIRLIFT), True
    if raw[F["enemy_foundry_known"]] == 0 and mask[SCOUT] and operation != SCOUT:
        return (production, construction, upgrade, SCOUT), True
    return plan, False


def doctrine_weighted_cross_entropy(
    logits: torch.Tensor,
    targets: torch.Tensor,
    class_weights: torch.Tensor,
    sample_weights: torch.Tensor,
    head_index: int,
) -> torch.Tensor:
    """masked_head_cross_entropy with a per-sample doctrine weight.

    The doubly-weighted mean reduces exactly to the class-weighted mean
    when every sample weight is 1, so an override weight of 1.0 is the
    unweighted teach bit for bit."""
    selected = logits.gather(-1, targets.unsqueeze(-1)).squeeze(-1)
    if not bool(torch.isfinite(selected).all().item()):
        bad_targets = torch.unique(targets[~torch.isfinite(selected)])
        raise ValueError(
            f"behavior-cloning targets are masked or non-finite in head {head_index}: "
            f"local classes {bad_targets.detach().cpu().tolist()}"
        )
    if bool((torch.isnan(logits) | torch.isposinf(logits)).any().item()):
        raise ValueError(
            f"behavior-cloning logits contain NaN or +inf in head {head_index}"
        )
    per_sample = torch.nn.functional.cross_entropy(
        logits, targets, weight=class_weights, reduction="none"
    )
    normalizer = (class_weights[targets] * sample_weights).sum()
    return (per_sample * sample_weights).sum() / normalizer


def anchored_head_loss(
    logits: torch.Tensor,
    anchor_logits: torch.Tensor,
    targets: torch.Tensor,
    class_weights: torch.Tensor,
    forced: torch.Tensor,
    head_index: int,
) -> torch.Tensor:
    """Cross entropy on doctrine-forced samples, KL to the founder's own
    distribution on natural ones.

    Cloning greedy self-play labels is argmax sharpening: every pass
    collapses the policy's rare-action tails toward its own most likely
    choice, which is exactly how repeated teaching erodes structures the
    fun gate later misses. Natural samples therefore anchor the student
    to the founder's full masked distribution instead of its argmax;
    only the forced samples carry a label to move toward."""
    anchor_log = torch.nn.functional.log_softmax(anchor_logits, dim=-1)
    anchor_prob = anchor_log.exp()
    student_log = torch.nn.functional.log_softmax(logits, dim=-1)
    # Masked classes carry -inf logs; zero them before the product so
    # neither the forward value nor the backward pass can meet a NaN.
    positive = anchor_prob > 0
    safe_anchor = torch.where(positive, anchor_log, torch.zeros_like(anchor_log))
    safe_student = torch.where(positive, student_log, torch.zeros_like(student_log))
    kl = (anchor_prob * (safe_anchor - safe_student)).sum(-1)
    # Every sample carries the same 1/batch weight as the plain teach:
    # averaging CE over the forced subset alone would silently multiply
    # the lesson dose by the batch-to-forced ratio (measured breaking
    # style at every dose until normalized here).
    per_sample = kl
    if forced.any():
        selected = logits[forced]
        chosen = selected.gather(-1, targets[forced].unsqueeze(-1)).squeeze(-1)
        if not bool(torch.isfinite(chosen).all().item()):
            raise ValueError(f"doctrine-forced targets are masked in head {head_index}")
        ce = torch.nn.functional.cross_entropy(
            selected, targets[forced], weight=class_weights, reduction="none"
        )
        per_sample = kl.clone()
        per_sample[forced] = ce
    return per_sample.mean()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--driver", default="../../target/release/oxide-driver")
    ap.add_argument("--resume", required=True, help="policy that plays AND learns")
    ap.add_argument("--episodes", type=int, default=48)
    ap.add_argument(
        "--scenario-dir",
        default="../../scenarios",
        help="directory whose shipped two-seat maps form the slate",
    )
    ap.add_argument("--epochs", type=int, default=4)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument(
        "--class-weight-cap",
        type=float,
        default=1.0,
        help="rare-label boost ceiling; 1.0 keeps the corpus's natural "
        "action frequencies (the override share IS the lesson dose)",
    )
    ap.add_argument(
        "--override-weight",
        type=float,
        default=1.0,
        help="loss multiplier on doctrine-forced samples; the natural "
        "play anchors style while the overrides carry the lesson",
    )
    ap.add_argument(
        "--anchor-natural",
        action="store_true",
        help="clone only the doctrine-forced labels; hold natural "
        "samples to the founder's full distribution by KL instead of "
        "sharpening toward its greedy argmax",
    )
    ap.add_argument(
        "--only-lesson",
        default=None,
        help="restrict collection to the profile entries teaching this "
        "lesson, concentrating the corpus on one behavior",
    )
    ap.add_argument(
        "--ferry-scenario",
        default=None,
        help="scenario stem whose episodes drill the ferry lesson; every "
        "other episode plays natural (no doctrine) so the corpus keys the "
        "shuttle to island-stall states instead of every open mask",
    )
    ap.add_argument(
        "--corpus",
        default=None,
        help="reuse a saved .npz corpus instead of collecting episodes",
    )
    ap.add_argument("--out", default="runs/night2/doctrine-bc.pt")
    args = ap.parse_args()

    torch.manual_seed(0)
    actor, blob = load_policy(args.resume)
    actor.eval()
    scenarios = duel_scenarios(pathlib.Path(args.scenario_dir))
    lessons = [*LESSONS, HUNT_LESSON, FERRY_LESSON]
    profile_table = PROFILE_LESSONS
    if args.only_lesson:
        profile_table = [e for e in PROFILE_LESSONS if e[2] == args.only_lesson]
        if not profile_table:
            raise SystemExit(f"no profile entry teaches {args.only_lesson!r}")
    if args.corpus:
        saved = np.load(args.corpus)
        obs_all = list(saved["obs"])
        mask_all = list(saved["mask"])
        act_all = [tuple(int(v) for v in row) for row in saved["act"]]
        if "forced_head" in saved.files:
            forced_all = [int(v) for v in saved["forced_head"]]
        elif "forced" in saved.files:
            # Boolean-era corpora recorded that a sample was forced but
            # not which head; -2 marks "forced somewhere" conservatively.
            forced_all = [-2 if v else -1 for v in saved["forced"]]
        else:
            forced_all = [-1] * len(act_all)
            if args.override_weight != 1.0:
                print("corpus predates forced flags; override weight is inert")
        print(f"reusing corpus {args.corpus}: {len(act_all)} samples")
        train(args, blob, obs_all, mask_all, act_all, forced_all, scenarios)
        return
    worker = Worker(args.driver)
    obs_all, mask_all, act_all = [], [], []
    forced_all: list[int] = []
    forced_total = 0
    lesson_forced = dict.fromkeys(lessons, 0)
    try:
        for ep in range(args.episodes):
            scenario = scenarios[ep % len(scenarios)]
            factions = FACTION_PAIRS[ep % len(FACTION_PAIRS)]
            entries = profile_table
            lesson_active = True
            if args.ferry_scenario:
                if scenario.stem == args.ferry_scenario:
                    entries = [e for e in PROFILE_LESSONS if e[2] == FERRY_LESSON]
                else:
                    lesson_active = False
            assignments = {
                seat: entries[(2 * ep + seat) % len(entries)] for seat in (0, 1)
            }
            seat_lessons: dict[int, str | None] = {
                seat: spec[2] if lesson_active else None
                for seat, spec in assignments.items()
            }
            catalog = worker.profile_catalog
            conds = {
                seat: catalog.condition(
                    assignments[seat][0],
                    assignments[seat][1],
                    catalog.default_role,
                    FACTION_NAME[factions[seat]],
                )
                for seat in (0, 1)
            }
            frame = worker.reset(
                31_000 + ep,
                control=(0, 1),
                scenario=str(scenario),
                conditions=conds,
                factions=factions,
                cadence=28,
            )
            while not frame.done:
                acts: dict[int, ActionPlan] = {}
                for seat, view in frame.seats.items():
                    with torch.no_grad():
                        logits, _ = actor(
                            torch.as_tensor(view.obs[None]),
                            torch.as_tensor(view.mask[None]),
                        )
                    greedy = factorized_greedy(logits)[0].cpu()
                    base: ActionPlan = (
                        int(greedy[0]),
                        int(greedy[1]),
                        int(greedy[2]),
                        int(greedy[3]),
                    )
                    lesson = seat_lessons[seat]
                    if lesson is None:
                        plan, forced = base, False
                    else:
                        plan, forced = apply_doctrine(base, lesson, view.raw, view.mask)
                        if forced:
                            forced_total += 1
                            lesson_forced[lesson] += 1
                    obs_all.append(view.obs)
                    mask_all.append(view.mask)
                    act_all.append(plan)
                    forced_all.append(
                        next(i for i in range(4) if plan[i] != base[i])
                        if forced
                        else -1
                    )
                    acts[seat] = plan
                frame = worker.step(acts)
    finally:
        worker.close()
    print(
        f"{len(act_all)} samples over {args.episodes} episodes; "
        f"{forced_total} doctrine overrides"
    )
    print(f"per-lesson overrides: {lesson_forced}")
    corpus_path = pathlib.Path(args.out).with_suffix(".npz")
    corpus_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(
        corpus_path,
        obs=np.stack(obs_all),
        mask=np.stack(mask_all),
        act=np.asarray(act_all, dtype=np.int64),
        forced_head=np.asarray(forced_all, dtype=np.int8),
    )
    print(f"corpus cached at {corpus_path}")
    train(args, blob, obs_all, mask_all, act_all, forced_all, scenarios)


def train(
    args: argparse.Namespace,
    blob: dict,
    obs_all: list,
    mask_all: list,
    act_all: list,
    forced_all: list,
    scenarios: list[pathlib.Path],
) -> None:
    obs = torch.as_tensor(np.stack(obs_all))
    mask = torch.as_tensor(np.stack(mask_all))
    forced_heads = torch.as_tensor(np.asarray(forced_all, dtype=np.int8))
    sample_weights = torch.ones(len(act_all), dtype=torch.float32)
    if args.override_weight != 1.0:
        sample_weights[forced_heads != -1] = args.override_weight
    local_targets = []
    class_weights = []
    for head, local in zip(ACTION_HEADS, local_action_targets(act_all), strict=True):
        counts = np.bincount(local, minlength=len(head))
        weights = np.sqrt(max(int(counts.max()), 1) / np.maximum(counts, 1))
        weights = np.minimum(weights, args.class_weight_cap)
        local_targets.append(torch.as_tensor(local))
        class_weights.append(torch.as_tensor(weights, dtype=torch.float32))

    policy, _ = load_policy(args.resume)
    anchor = None
    if args.anchor_natural:
        anchor, _ = load_policy(args.resume)
        anchor.eval()
        for parameter in anchor.parameters():
            parameter.requires_grad_(False)
    training_dir = pathlib.Path(__file__).resolve().parent
    run_lineage = build_lineage(
        phase="behavior-cloning",
        phase_start_update=int(blob.get("update", 0) or 0),
        hyperparameters={
            "arch": blob.get("arch"),
            "batch_size": 1024,
            "class_weighting": "inverse-square-root-cap-10",
            "corpus": "doctrine-self-play",
            "episodes": args.episodes,
            "episode_seed_base": 31_000,
            "epochs": args.epochs,
            "gym_version": GYM_VERSION,
            "learning_rate": args.lr,
            "lessons": {name: lesson[2] for name, lesson in LESSONS.items()},
            "natural_anchor": bool(args.anchor_natural),
            "override_weight": args.override_weight,
            "profile_lessons": PROFILE_LESSONS,
            "scenario_content_sha256": [content_digest(s) for s in scenarios],
            "torch_seed": 0,
        },
        inputs={
            "gym_client": input_identity(training_dir / "oxide_gym.py"),
            "gym_driver": input_identity(args.driver),
            "model_code": input_identity(training_dir / "models.py"),
            "trainer": input_identity(training_dir / "doctrine_bc.py"),
            "initializer": input_identity(args.resume, blob),
        },
    )
    opt = torch.optim.Adam(policy.parameters(), lr=args.lr)
    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    n = len(act_all)
    for epoch in range(args.epochs):
        perm = torch.randperm(n)
        total = 0.0
        for start in range(0, n, 1024):
            mb = perm[start : start + 1024]
            logits, _ = policy(obs[mb], mask[mb])
            if anchor is not None:
                with torch.no_grad():
                    anchor_logits, _ = anchor(obs[mb], mask[mb])
            losses = []
            for head_index, head in enumerate(ACTION_HEADS):
                indices = torch.as_tensor(head)
                if anchor is not None:
                    batch_heads = forced_heads[mb]
                    head_forced = (batch_heads == head_index) | (batch_heads == -2)
                    losses.append(
                        anchored_head_loss(
                            logits.index_select(-1, indices),
                            anchor_logits.index_select(-1, indices),
                            local_targets[head_index][mb],
                            class_weights[head_index],
                            head_forced,
                            head_index,
                        )
                    )
                    continue
                losses.append(
                    doctrine_weighted_cross_entropy(
                        logits.index_select(-1, indices),
                        local_targets[head_index][mb],
                        class_weights[head_index],
                        sample_weights[mb],
                        head_index,
                    )
                )
            loss = torch.stack(losses).mean()
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += float(loss.detach()) * len(mb)
        print(f"epoch {epoch}: loss {total / n:.4f}")
    save_policy(
        policy,
        str(blob.get("arch") or "mlp"),
        args.out,
        {"update": blob.get("update"), "lineage": run_lineage},
    )
    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
