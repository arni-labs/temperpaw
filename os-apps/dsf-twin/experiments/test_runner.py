"""The shipped runner's identity and durable receipt boundaries."""

import json
import subprocess
import sys
from pathlib import Path

import pytest
from runner import Manifest, ReceiptStore, Refused


def manifest() -> dict[str, object]:
    return {
        "version": 1,
        "experiment_id": "variant-a",
        "effort_id": "effort-1",
        "computer_id": "arni-big",
        "branch": "codex/arn467-variant-a",
        "source_revision": "a" * 40,
        "runner_sha256": "a" * 64,
        "database_id": "dsf_variant_a",
        "media_bucket": "dsf-variant-a",
        "media_namespace": "experiments/variant-a/",
        "permitted_external_calls": [],
        "cors_origin": "https://variant-a.invalid",
        "production_database_id": "production-database",
        "production_media_bucket": "production-media",
    }


@pytest.mark.parametrize(
    "field,value",
    [
        ("experiment_id", "../production"),
        ("source_revision", "main"),
        ("database_id", "production-database"),
        ("media_bucket", "production-media"),
        ("permitted_external_calls", ["https://api.openai.com"]),
        ("branch", "main"),
        ("media_namespace", "production/"),
    ],
)
def test_manifest_refuses_unbound_or_paid_targets(field: str, value: object) -> None:
    raw = manifest()
    raw[field] = value
    with pytest.raises(Refused):
        Manifest.parse(raw)


def test_receipt_replay_survives_restart_and_rejects_changed_manifest(
    tmp_path: Path,
) -> None:
    bound = Manifest.parse(manifest())
    with ReceiptStore(tmp_path, bound, "validate") as store:
        assert store.read() is None
        store.commit({"outcome": "passed", "database_oid": "16422"})
    with ReceiptStore(tmp_path, bound, "validate") as store:
        assert store.read()["database_oid"] == "16422"
    changed = manifest()
    changed["cors_origin"] = "https://changed.invalid"
    with (
        pytest.raises(Refused),
        ReceiptStore(tmp_path, Manifest.parse(changed), "validate"),
    ):
        pass


def test_cleanup_never_adopts_existing_directory(tmp_path: Path) -> None:
    (tmp_path / "variant-a").mkdir()
    (tmp_path / "variant-a" / "unowned").write_text("keep")
    with (
        pytest.raises(Refused),
        ReceiptStore(tmp_path, Manifest.parse(manifest()), "cleanup"),
    ):
        pass
    assert (tmp_path / "variant-a" / "unowned").read_text() == "keep"


def test_committed_result_survives_process_death_before_response(
    tmp_path: Path,
) -> None:
    raw = json.dumps(manifest())
    code = """import json,os,sys
from pathlib import Path
from runner import Manifest,ReceiptStore
with ReceiptStore(Path(sys.argv[1]),Manifest.parse(json.loads(sys.argv[2])),"run") as store:
    store.commit({"outcome":"passed","story_id":"one-created-story"})
    os._exit(17)
"""
    result = subprocess.run(
        [sys.executable, "-c", code, str(tmp_path), raw],
        cwd=Path(__file__).parent,
        check=False,
        timeout=5,
    )
    assert result.returncode == 17
    with ReceiptStore(tmp_path, Manifest.parse(manifest()), "run") as store:
        saved = store.read()
        assert saved is not None and saved["story_id"] == "one-created-story"


def test_archive_is_reproducible(tmp_path: Path) -> None:
    from build_runner import build

    assert build(tmp_path / "first.pyz") == build(tmp_path / "second.pyz")


def test_manifest_has_no_arbitrary_endpoint_or_command_field() -> None:
    raw = manifest()
    raw["command"] = "arbitrary provider call"
    with pytest.raises(Refused):
        Manifest.parse(raw)
