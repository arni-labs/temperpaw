"""Reject DSF short-name collisions before publishing or installing a bundle.

The target metadata must be fetched from the target tenant immediately before
installation. An upgrade also needs the previous installed bundle and its pinned
installation record. Namespace equality alone never establishes ownership.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path

EDM = "{http://docs.oasis-open.org/odata/ns/edm}"


@dataclass(frozen=True)
class Names:
    types: frozenset[str]
    sets: frozenset[str]
    duplicate_types: frozenset[str] = frozenset()
    duplicate_sets: frozenset[str] = frozenset()


def names(model: bytes, *, candidate: bool) -> Names:
    """Collect short names across every schema, refusing internal duplicates."""
    if len(model) > 4 * 1024 * 1024:
        raise ValueError("Metadata exceeds 4 MiB")
    root = ET.fromstring(model)
    groups: dict[str, set[str]] = {"EntityType": set(), "EntitySet": set()}
    duplicates: dict[str, set[str]] = {"EntityType": set(), "EntitySet": set()}
    for kind, values in groups.items():
        for node in root.iter(EDM + kind):
            name = node.get("Name", "")
            if not name or (candidate and name in values):
                raise ValueError(f"Duplicate or empty {kind} short name: {name}")
            if name in values:
                duplicates[kind].add(name)
            if candidate and not name.startswith("Dsf"):
                raise ValueError(f"DSF cannot declare an unprefixed {kind}: {name}")
            values.add(name)
    if not groups["EntityType"] or not groups["EntitySet"]:
        raise ValueError("Metadata requires entity types and entity sets")
    return Names(
        frozenset(groups["EntityType"]),
        frozenset(groups["EntitySet"]),
        frozenset(duplicates["EntityType"]),
        frozenset(duplicates["EntitySet"]),
    )


def check(candidate: Names, live: Names, owned: Names | None = None) -> None:
    """Existing names require explicit previous-bundle ownership evidence."""
    owned = owned or Names(frozenset(), frozenset())
    if candidate.types & live.duplicate_types or candidate.sets & live.duplicate_sets:
        raise ValueError("Target metadata has ambiguous ownership for a DSF name")
    if not owned.types <= live.types or not owned.sets <= live.sets:
        raise ValueError("Previous installed bundle does not match target metadata")
    types = (candidate.types & live.types) - owned.types
    sets = (candidate.sets & live.sets) - owned.sets
    if types or sets:
        raise ValueError(
            f"Names already owned outside dsf-twin: types={sorted(types)}, sets={sorted(sets)}"
        )


def previous_names(record_path: Path, model_path: Path, tenant: str) -> Names:
    """Bind a prior model to an operator-exported pinned installation record.

    The publisher obtains this record from the running tenant's installed app
    store. A repository branch, current namespace or candidate bundle cannot
    substitute for that export.
    """
    record: object = json.loads(record_path.read_text())
    if not isinstance(record, dict):
        raise TypeError("Installed app record must be an object")
    if record.get("tenant") != tenant or record.get("app_name") != "dsf-twin":
        raise ValueError("Installed app record belongs to another tenant or app")
    ref = record.get("app_ref")
    if (
        not isinstance(ref, str)
        or re.fullmatch(r"[^/@]+/dsf-twin@[a-f0-9]{40,64}", ref) is None
    ):
        raise ValueError("Installed app record requires a pinned Genesis ref")
    model = model_path.read_bytes()
    if record.get("model_sha256") != hashlib.sha256(model).hexdigest():
        raise ValueError("Previous model does not match installed app export")
    return names(model, candidate=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--model", type=Path, default=Path(__file__).parent / "specs/model.csdl.xml"
    )
    parser.add_argument("--live-metadata", type=Path, required=True)
    parser.add_argument("--tenant", required=True)
    parser.add_argument("--installed-record", type=Path)
    parser.add_argument("--installed-model", type=Path)
    args = parser.parse_args()
    if bool(args.installed_record) != bool(args.installed_model):
        parser.error("An upgrade requires both the installed record and its model")
    owned = (
        previous_names(args.installed_record, args.installed_model, args.tenant)
        if args.installed_record
        else None
    )
    model = args.model.read_bytes()
    live = args.live_metadata.read_bytes()
    candidate = names(model, candidate=True)
    inventory = names(live, candidate=False)
    check(candidate, inventory, owned)
    print(
        json.dumps(
            {
                "app": "dsf-twin",
                "tenant": args.tenant,
                "entity_types": sorted(candidate.types),
                "entity_sets": sorted(candidate.sets),
                "model_sha256": hashlib.sha256(model).hexdigest(),
                "live_metadata_sha256": hashlib.sha256(live).hexdigest(),
                "collisions": 0,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
