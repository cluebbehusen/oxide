"""Content-addressed provenance for training phases and their artifacts."""

from __future__ import annotations

import hashlib
import json
import pathlib
from typing import TYPE_CHECKING, cast

if TYPE_CHECKING:
    from collections.abc import Mapping

LINEAGE_SCHEMA = 1
_HASH_PREFIX = "sha256:"


def _is_digest(value: object) -> bool:
    if not isinstance(value, str) or not value.startswith(_HASH_PREFIX):
        return False
    hexadecimal = value.removeprefix(_HASH_PREFIX)
    return len(hexadecimal) == 64 and all(
        character in "0123456789abcdef" for character in hexadecimal
    )


def content_digest(path: str | pathlib.Path) -> str:
    """Returns a path-independent SHA-256 identity for one file."""
    digest = hashlib.sha256()
    with pathlib.Path(path).open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return _HASH_PREFIX + digest.hexdigest()


def _canonical_json(value: object) -> str:
    try:
        return json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=True,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError) as err:
        raise ValueError(
            "lineage metadata must contain only finite JSON values"
        ) from err


def _payload_id(payload: Mapping[str, object]) -> str:
    encoded = _canonical_json(payload).encode()
    return _HASH_PREFIX + hashlib.sha256(encoded).hexdigest()


def validate_lineage(value: object) -> dict[str, object]:
    """Validates and returns a canonical copy of a lineage manifest."""
    if not isinstance(value, dict):
        raise TypeError("lineage metadata must be an object")
    if not all(isinstance(key, str) for key in value):
        raise TypeError("lineage metadata keys must be strings")
    typed_value = cast("dict[str, object]", value)
    lineage_id = typed_value.get("lineage_id")
    if not isinstance(lineage_id, str):
        raise TypeError("lineage metadata must carry a lineage_id")
    payload = {key: item for key, item in typed_value.items() if key != "lineage_id"}
    if payload.get("schema") != LINEAGE_SCHEMA:
        raise ValueError(
            f"unsupported lineage schema {payload.get('schema')!r}; "
            f"expected {LINEAGE_SCHEMA}"
        )
    if not _is_digest(lineage_id):
        raise ValueError("lineage_id must be a SHA-256 digest")
    if not isinstance(payload.get("phase"), str) or not payload["phase"]:
        raise ValueError("lineage phase must be a non-empty string")
    phase_start = payload.get("phase_start_update")
    if (
        not isinstance(phase_start, int)
        or isinstance(phase_start, bool)
        or phase_start < 0
    ):
        raise ValueError("lineage phase_start_update must be a non-negative integer")
    if not isinstance(payload.get("hyperparameters"), dict):
        raise TypeError("lineage hyperparameters must be an object")
    inputs = payload.get("inputs")
    if not isinstance(inputs, dict):
        raise TypeError("lineage inputs must be an object")
    for role, identity in inputs.items():
        if not isinstance(role, str) or not role:
            raise ValueError("lineage input roles must be non-empty strings")
        if not isinstance(identity, dict):
            raise TypeError(f"lineage input {role!r} must be an object")
        if not _is_digest(identity.get("content_sha256")):
            raise ValueError(
                f"lineage input {role!r} must carry a SHA-256 content digest"
            )
        upstream_id = identity.get("lineage_id")
        if upstream_id is not None and not _is_digest(upstream_id):
            raise ValueError(
                f"lineage input {role!r} carries an invalid upstream lineage id"
            )
    if _payload_id(payload) != lineage_id:
        raise ValueError("lineage_id does not match the lineage manifest")
    canonical = json.loads(_canonical_json(typed_value))
    if not isinstance(canonical, dict):
        raise TypeError("a lineage object canonicalized to a non-object")
    return canonical


def inherited_lineage_id(metadata: Mapping[str, object] | None) -> str | None:
    """Returns a verified upstream lineage id from checkpoint metadata."""
    if metadata is None or "lineage" not in metadata:
        return None
    lineage = validate_lineage(metadata["lineage"])
    lineage_id = lineage["lineage_id"]
    if not isinstance(lineage_id, str):
        raise TypeError("validated lineage has a non-string id")
    return lineage_id


def input_identity(
    path: str | pathlib.Path,
    metadata: Mapping[str, object] | None = None,
) -> dict[str, object]:
    """Identifies an input by bytes and, when available, its lineage."""
    identity: dict[str, object] = {"content_sha256": content_digest(path)}
    upstream_id = inherited_lineage_id(metadata)
    if upstream_id is not None:
        identity["lineage_id"] = upstream_id
    return identity


def build_lineage(
    *,
    phase: str,
    phase_start_update: int,
    hyperparameters: Mapping[str, object],
    inputs: Mapping[str, Mapping[str, object]] | None = None,
) -> dict[str, object]:
    """Builds a stable phase identity from content, not filesystem paths."""
    if not phase:
        raise ValueError("lineage phase must not be empty")
    if phase_start_update < 0:
        raise ValueError("phase_start_update must be non-negative")
    payload: dict[str, object] = {
        "schema": LINEAGE_SCHEMA,
        "phase": phase,
        "phase_start_update": phase_start_update,
        "hyperparameters": dict(hyperparameters),
        "inputs": {
            role: dict(identity)
            for role, identity in (inputs.items() if inputs is not None else ())
        },
    }
    canonical = json.loads(_canonical_json(payload))
    if not isinstance(canonical, dict):
        raise TypeError("a lineage payload canonicalized to a non-object")
    canonical["lineage_id"] = _payload_id(canonical)
    return validate_lineage(canonical)


def checkpoint_metadata(
    lineage: Mapping[str, object],
    fields: Mapping[str, object],
) -> dict[str, object]:
    """Attaches a verified lineage manifest to checkpoint fields."""
    return {**fields, "lineage": validate_lineage(dict(lineage))}


def export_lineage(metadata: Mapping[str, object]) -> dict[str, object] | None:
    """Returns verified lineage metadata suitable for a Q12 artifact."""
    value = metadata.get("lineage")
    return None if value is None else validate_lineage(value)
