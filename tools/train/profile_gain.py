"""Apply an exact rational gain to one learned profile input column.

This is a narrow promotion calibration, not another policy-training phase.
Every coefficient outside the selected first-layer column remains byte-exact,
and the transform records its source artifact, rational gain, and this script
in a content-addressed lineage manifest.

Usage (from tools/train/):
    uv run profile_gain.py \
      --src runs/profile/probe/weights-00210.json \
      --out runs/profile/promoted.json \
      --column profile_siege --numerator 2
"""

import argparse
import copy
import json
import pathlib

from lineage import build_lineage, input_identity
from oxide_gym import CONDITION_NAMES, FEATURES, GYM_VERSION

PROFILE_CONDITION_START = CONDITION_NAMES.index("profile_economy")
PROFILE_CONDITION_NAMES = CONDITION_NAMES[PROFILE_CONDITION_START:]
MAX_COEFF = 1 << 20


def positive_int(text: str) -> int:
    """Argparse type for a strictly positive integer."""
    value = int(text)
    if value <= 0:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got {text!r}")
    return value


def round_ratio(value: int, numerator: int, denominator: int) -> int:
    """Scales an integer, rounding nearest with exact ties away from zero."""
    scaled = value * numerator
    magnitude = (abs(scaled) + denominator // 2) // denominator
    return -magnitude if scaled < 0 else magnitude


def apply_profile_gain(
    artifact: dict,
    column: str,
    numerator: int,
    denominator: int = 1,
) -> dict:
    """Returns a copy with only one Q12 profile column scaled."""
    if numerator <= 0 or denominator <= 0:
        raise ValueError("profile gain terms must be positive")
    if column not in PROFILE_CONDITION_NAMES:
        raise ValueError(f"unknown profile column {column!r}")
    if artifact.get("gym_version") != GYM_VERSION:
        actual_version = artifact.get("gym_version")
        raise ValueError(
            f"profile gain expects gym v{GYM_VERSION}, got {actual_version!r}"
        )
    if artifact.get("features") != FEATURES:
        raise ValueError("artifact feature count does not match the gym contract")
    if artifact.get("conditioning") != len(CONDITION_NAMES):
        raise ValueError("artifact conditioning count does not match the gym contract")
    layers = artifact.get("layers")
    if not isinstance(layers, list) or not layers:
        raise ValueError("artifact has no first layer")
    weights = layers[0].get("w") if isinstance(layers[0], dict) else None
    expected_width = FEATURES + len(CONDITION_NAMES)
    if not isinstance(weights, list) or not weights:
        raise ValueError("artifact first layer has no weights")
    if any(not isinstance(row, list) or len(row) != expected_width for row in weights):
        raise ValueError("artifact first-layer width does not match the gym contract")

    calibrated = copy.deepcopy(artifact)
    column_index = FEATURES + CONDITION_NAMES.index(column)
    for row in calibrated["layers"][0]["w"]:
        row[column_index] = round_ratio(
            row[column_index],
            numerator,
            denominator,
        )
        if abs(row[column_index]) > MAX_COEFF:
            raise ValueError(f"calibrated coefficient exceeds +/-{MAX_COEFF}")
    return calibrated


def calibration_lineage(
    source: str | pathlib.Path,
    metadata: dict,
    column: str,
    numerator: int,
    denominator: int,
) -> dict[str, object]:
    """Builds the content-addressed calibration provenance."""
    return build_lineage(
        phase="profile-column-gain",
        phase_start_update=int(metadata.get("update", 0) or 0),
        hyperparameters={
            "column": column,
            "denominator": denominator,
            "numerator": numerator,
            "rounding": "nearest-ties-away-from-zero",
        },
        inputs={
            "source": input_identity(source, metadata),
            "transformer_code": input_identity(pathlib.Path(__file__)),
        },
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--column", required=True, choices=PROFILE_CONDITION_NAMES)
    ap.add_argument("--numerator", required=True, type=positive_int)
    ap.add_argument("--denominator", default=1, type=positive_int)
    args = ap.parse_args()

    source = pathlib.Path(args.src)
    output = pathlib.Path(args.out)
    if source.resolve() == output.resolve():
        ap.error("--out must not overwrite --src")
    with source.open() as handle:
        artifact = json.load(handle)
    calibrated = apply_profile_gain(
        artifact,
        args.column,
        args.numerator,
        args.denominator,
    )
    calibrated["lineage"] = calibration_lineage(
        source,
        artifact,
        args.column,
        args.numerator,
        args.denominator,
    )
    with output.open("w") as handle:
        json.dump(calibrated, handle)
        handle.write("\n")
    print(f"calibrated {args.column} by {args.numerator}/{args.denominator}: {output}")


if __name__ == "__main__":
    main()
