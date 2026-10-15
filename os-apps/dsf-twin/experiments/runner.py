"""Bounded DSF experiments. Invoked by governed Exec inside a fresh network namespace."""

from __future__ import annotations

import argparse
import base64
import fcntl
import hashlib
import json
import os
import re
import secrets
import select
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from contextlib import AbstractContextManager, contextmanager
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import IO, BinaryIO, Literal, Protocol, Self, cast

ROOT = Path("/home/tl-user/work/arn467-experiments")
PG = Path("/usr/lib/postgresql/16/bin")
Phase = Literal["validate", "run", "cleanup"]


class Refused(RuntimeError):
    """A target or retained result does not belong to this experiment."""


@dataclass(frozen=True)
class Manifest:
    version: int
    experiment_id: str
    effort_id: str
    computer_id: str
    branch: str
    source_revision: str
    runner_sha256: str
    database_id: str
    media_bucket: str
    media_namespace: str
    permitted_external_calls: tuple[str, ...]
    cors_origin: str
    production_database_id: str
    production_media_bucket: str

    @classmethod
    def parse(cls, raw: object) -> Manifest:
        if not isinstance(raw, dict) or set(raw) != set(cls.__dataclass_fields__):
            raise Refused("manifest fields do not match version 1")
        if raw["version"] != 1 or raw["permitted_external_calls"] != []:
            raise Refused("this runner permits no external calls")
        for key, value in raw.items():
            if key not in {"version", "permitted_external_calls"} and (
                not isinstance(value, str) or not value or len(value) > 200
            ):
                raise Refused(f"invalid manifest field {key}")
        ident = raw["experiment_id"]
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,39}", ident):
            raise Refused("invalid experiment ID")
        if raw["computer_id"] != "arni-big":
            raise Refused("runner is installed only on arni-big")
        if not re.fullmatch(r"[a-f0-9]{64}", raw["runner_sha256"]):
            raise Refused("runner must be a pinned SHA256 archive")
        if not re.fullmatch(r"[a-f0-9]{40}", raw["source_revision"]):
            raise Refused("source must be a full Git SHA")
        if raw["branch"] != f"codex/arn467-{ident}":
            raise Refused("branch must belong to this experiment")
        if raw["database_id"] != "dsf_" + ident.replace("-", "_"):
            raise Refused("database name must belong to this experiment")
        if (
            raw["media_bucket"] != f"dsf-{ident}"
            or raw["media_namespace"] != f"experiments/{ident}/"
        ):
            raise Refused("media target must belong to this experiment")
        if (
            raw["database_id"] == raw["production_database_id"]
            or raw["media_bucket"] == raw["production_media_bucket"]
        ):
            raise Refused("production bindings are prohibited")
        if not re.fullmatch(
            r"https://[a-z][a-z0-9-]{0,60}\.invalid", raw["cors_origin"]
        ):
            raise Refused("variant must use an isolated test origin")
        return cls(**{**raw, "permitted_external_calls": ()})

    @property
    def digest(self) -> str:
        return hashlib.sha256(
            json.dumps(asdict(self), sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()


def atomic_json(path: Path, value: object) -> None:
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w") as stream:
        os.chmod(temporary, 0o600)
        json.dump(value, stream, sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class ReceiptStore(AbstractContextManager["ReceiptStore"]):
    """One lock for every phase; completed receipts survive commands and API restarts."""

    def __init__(self, root: Path, manifest: Manifest, phase: Phase):
        self.directory = root / manifest.experiment_id
        self.manifest = manifest
        self.phase = phase
        self.lock: IO[str] | None = None

    def __enter__(self) -> Self:
        self.directory.parent.mkdir(parents=True, exist_ok=True)
        if self.directory.is_symlink():
            raise Refused("experiment directory is a symlink")
        try:
            self.directory.mkdir(mode=0o700)
            atomic_json(
                self.directory / "owner.json", {"manifest_sha256": self.manifest.digest}
            )
        except FileExistsError:
            if not (self.directory / "owner.json").is_file():
                raise Refused("existing directory has no ownership marker") from None
        owner = json.loads((self.directory / "owner.json").read_text())
        if owner != {"manifest_sha256": self.manifest.digest}:
            raise Refused("existing directory belongs to another manifest")
        self.lock = (self.directory / "receipt.lock").open("a+")
        fcntl.flock(self.lock, fcntl.LOCK_EX)
        return self

    def __exit__(self, *args: object) -> None:
        if self.lock is not None:
            self.lock.close()

    def read(self) -> dict[str, object] | None:
        receipt = self.directory / f"{self.phase}.json"
        return (
            cast(dict[str, object], json.loads(receipt.read_text()))
            if receipt.exists()
            else None
        )

    def commit(self, result: dict[str, object]) -> dict[str, object]:
        value = {
            **result,
            "manifest_sha256": self.manifest.digest,
            "phase": self.phase,
            "experiment_id": self.manifest.experiment_id,
            "source_revision": self.manifest.source_revision,
        }
        atomic_json(self.directory / f"{self.phase}.json", value)
        return value


def command(
    args: list[str | Path],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> str:
    result = subprocess.run(
        [str(arg) for arg in args],
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=180,
        check=False,
    )
    if result.returncode:
        # Logs stay on the private computer; exception output never contains environment values.
        raise Refused(f"{Path(str(args[0])).name} failed with exit {result.returncode}")
    return result.stdout.strip()


def safe_environment(directory: Path) -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": str(directory),
        "LANG": "C.UTF-8",
        "PYTHONUNBUFFERED": "1",
        "PYTHONDONTWRITEBYTECODE": "1",
    }


def assert_network_isolation() -> None:
    # sysfs can retain the host mount's network view after unshare; query the namespace directly.
    interfaces = {name for _, name in socket.if_nameindex()}
    routes = Path("/proc/net/route").read_text().splitlines()[1:]
    if interfaces != {"lo"} or any(line.split()[1] != "00000000" for line in routes):
        raise Refused("runner requires a fresh network namespace with loopback only")
    # A default route is also forbidden, even if it points at loopback.
    if any(line.split()[1] == "00000000" for line in routes):
        raise Refused("runner has an external route")


def checkout(store: ReceiptStore) -> Path:
    manifest = store.manifest
    target = store.directory / "source"
    repository = ROOT / "repository.git"
    if not target.exists():
        command(
            [
                "git",
                "--git-dir",
                repository,
                "worktree",
                "add",
                "-b",
                manifest.branch,
                target,
                manifest.source_revision,
            ]
        )
    actual_sha = command(["git", "rev-parse", "HEAD"], cwd=target)
    actual_branch = command(["git", "symbolic-ref", "--short", "HEAD"], cwd=target)
    if actual_sha != manifest.source_revision or actual_branch != manifest.branch:
        raise Refused("checkout branch or revision changed")
    if command(["git", "status", "--porcelain", "--untracked-files=all"], cwd=target):
        raise Refused("checkout contains unrecorded changes")
    return target


def psql(store: ReceiptStore, sql: str, database: str = "postgres") -> str:
    return command(
        [
            PG / "psql",
            "-h",
            "127.0.0.1",
            "-p",
            "55432",
            "-U",
            "experiment",
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
            "-c",
            sql,
        ]
    )


class S3(Protocol):
    """The bounded storage API used at the provider boundary."""

    def list_buckets(self) -> dict[str, list[dict[str, str]]]: ...
    def create_bucket(self, *, Bucket: str) -> object: ...
    def put_object(self, *, Bucket: str, Key: str, Body: bytes) -> object: ...
    def get_object(self, *, Bucket: str, Key: str) -> dict[str, BinaryIO]: ...


def reconcile_processes(store: ReceiptStore) -> None:
    """Reap only orphaned processes whose exact command and cwd identify our artifacts."""
    directory = store.directory
    for process in Path("/proc").iterdir():
        if not process.name.isdigit() or int(process.name) == os.getpid():
            continue
        descriptor = None
        try:
            if process.stat().st_uid != os.getuid():
                continue
            descriptor = os.pidfd_open(int(process.name))
            args = (process / "cmdline").read_bytes().decode().rstrip("\0").split("\0")
            owned_pg = (
                len(args) >= 3
                and args[0] == str(PG / "postgres")
                and args[1:3] == ["-D", str(directory / "postgres")]
            )
            owned_media = len(args) >= 3 and args[:3] == [
                str(ROOT / "tools/minio"),
                "server",
                str(directory / "media"),
            ]
            owned_api = (
                len(args) >= 4
                and args[:4]
                == [str(ROOT / "tools/venv/bin/python"), "-m", "uvicorn", "main:app"]
                and (process / "cwd").resolve() == directory / "source/platform/backend"
            )
            if not (owned_pg or owned_media or owned_api):
                continue
            signal.pidfd_send_signal(
                descriptor, signal.SIGINT if owned_pg else signal.SIGTERM
            )
            poll = select.poll()
            poll.register(descriptor, select.POLLIN)
            if not poll.poll(10000):
                signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                if not poll.poll(5000):
                    raise Refused("owned service did not stop")
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            # A disappeared process is reconciled; other users' processes are never adopted.
            continue
        finally:
            if descriptor is not None:
                os.close(descriptor)
    pid_file = directory / "postgres/postmaster.pid"
    if pid_file.exists():
        old_pid = pid_file.read_text().splitlines()[0]
        if not old_pid.isdigit() or Path("/proc", old_pid).exists():
            raise Refused(
                "database pid still belongs to a live or unrecognized process"
            )
        pid_file.unlink()


@contextmanager
def services(store: ReceiptStore) -> Iterator[S3]:
    """Services die with the bounded phase; only owned data directories persist."""
    import boto3  # Third-party runtime is installed into the isolated tool venv.
    from botocore.config import Config

    directory = store.directory
    reconcile_processes(store)
    data = directory / "postgres"
    env = safe_environment(directory)
    if data.is_symlink() or (directory / "media").is_symlink():
        raise Refused("service data path is a symlink")
    if not (data / "PG_VERSION").exists():
        if data.exists():
            # Only our ownership-marked directory can reach here; initdb is retryable.
            shutil.rmtree(data)
        command(
            [
                PG / "initdb",
                "-D",
                data,
                "-U",
                "experiment",
                "--auth=trust",
                "--no-locale",
            ],
            env=env,
        )
    command(
        [
            PG / "pg_ctl",
            "-D",
            data,
            "-l",
            directory / "postgres.log",
            "-w",
            "start",
            "-o",
            "-h 127.0.0.1 -p 55432 -k ''",
        ],
        env=env,
    )
    key_path = directory / "media-credentials.json"
    if not key_path.exists():
        atomic_json(key_path, {"access": "experiment", "secret": secrets.token_hex(24)})
    keys = json.loads(key_path.read_text())
    minio_env = {
        **env,
        "MINIO_ROOT_USER": keys["access"],
        "MINIO_ROOT_PASSWORD": keys["secret"],
        "MINIO_BROWSER": "off",
        "MINIO_UPDATE": "off",
    }
    with (directory / "minio.log").open("ab") as log:
        process = subprocess.Popen(
            [
                str(ROOT / "tools/minio"),
                "server",
                str(directory / "media"),
                "--address",
                "127.0.0.1:59000",
            ],
            env=minio_env,
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                try:
                    with urllib.request.urlopen(
                        "http://127.0.0.1:59000/minio/health/live", timeout=1
                    ) as response:
                        if response.status == 200:
                            break
                except (OSError, urllib.error.URLError):
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise Refused("isolated media server did not start") from None
                    time.sleep(0.1)
            client = boto3.client(
                "s3",
                endpoint_url="http://127.0.0.1:59000",
                aws_access_key_id=keys["access"],
                aws_secret_access_key=keys["secret"],
                region_name="us-east-1",
                config=Config(
                    connect_timeout=2, read_timeout=5, retries={"max_attempts": 0}
                ),
            )
            yield client
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            command([PG / "pg_ctl", "-D", data, "-m", "fast", "-w", "stop"], env=env)


def validate(store: ReceiptStore) -> dict[str, object]:
    source = checkout(store)
    manifest = store.manifest
    with services(store) as media:
        database = manifest.database_id
        if not psql(store, f"SELECT 1 FROM pg_database WHERE datname='{database}'"):
            psql(store, f'CREATE DATABASE "{database}"')
        psql(store, "CREATE EXTENSION IF NOT EXISTS vector", database)
        identity = psql(store, "SELECT system_identifier FROM pg_control_system()")
        oid = psql(
            store,
            "SELECT oid FROM pg_database WHERE datname=current_database()",
            database,
        )
        version = psql(
            store,
            "SELECT extversion FROM pg_extension WHERE extname='vector'",
            database,
        )
        owner = manifest.media_namespace + "owner.json"
        buckets = {bucket["Name"] for bucket in media.list_buckets()["Buckets"]}
        if manifest.media_bucket not in buckets:
            media.create_bucket(Bucket=manifest.media_bucket)
            media.put_object(
                Bucket=manifest.media_bucket,
                Key=owner,
                Body=json.dumps({"manifest_sha256": manifest.digest}).encode(),
            )
        saved = json.loads(
            media.get_object(Bucket=manifest.media_bucket, Key=owner)["Body"].read()
        )
        if saved != {"manifest_sha256": manifest.digest}:
            raise Refused("bucket ownership marker does not match")
    return {
        "outcome": "passed",
        "database_system_identifier": identity,
        "database_oid": oid,
        "database_id": database,
        "pgvector_version": version,
        "media_bucket": manifest.media_bucket,
        "media_namespace": manifest.media_namespace,
        "branch": manifest.branch,
        "source_path": str(source),
        "network_interfaces": ["lo"],
        "external_routes": [],
        "external_calls": [],
        "production_database_id": manifest.production_database_id,
        "production_media_bucket": manifest.production_media_bucket,
    }


def run(store: ReceiptStore) -> dict[str, object]:
    from product_flow import verify

    isolation_path = store.directory / "validate.json"
    if not isolation_path.exists():
        raise Refused("validation receipt is missing")
    isolation = json.loads(isolation_path.read_text())
    if (
        isolation["manifest_sha256"] != store.manifest.digest
        or isolation["outcome"] != "passed"
    ):
        raise Refused("validation receipt does not match")
    source = checkout(store)
    with services(store) as media:
        if (
            psql(store, "SELECT system_identifier FROM pg_control_system()")
            != isolation["database_system_identifier"]
        ):
            raise Refused("database identity changed")
        marker = json.loads(
            media.get_object(
                Bucket=store.manifest.media_bucket,
                Key=store.manifest.media_namespace + "owner.json",
            )["Body"].read()
        )
        if marker != {"manifest_sha256": store.manifest.digest}:
            raise Refused("media ownership changed")
        return verify(store, source)


def cleanup(store: ReceiptStore) -> dict[str, object]:
    # The receipt lock and immutable manifest are checked before every deletion.
    reconcile_processes(store)
    target = store.directory / "source"
    if target.exists():
        checkout(store)
        command(
            [
                "git",
                "--git-dir",
                ROOT / "repository.git",
                "worktree",
                "remove",
                str(target),
            ]
        )
    branch = store.manifest.branch
    exists = (
        subprocess.run(
            [
                "git",
                "--git-dir",
                str(ROOT / "repository.git"),
                "show-ref",
                "--verify",
                "--quiet",
                f"refs/heads/{branch}",
            ],
            check=False,
        ).returncode
        == 0
    )
    if exists:
        actual = command(
            ["git", "--git-dir", ROOT / "repository.git", "rev-parse", branch]
        )
        if actual != store.manifest.source_revision:
            raise Refused("cleanup branch revision changed")
        command(["git", "--git-dir", ROOT / "repository.git", "branch", "-D", branch])
    for name in ("postgres", "media"):
        target = store.directory / name
        if target.is_symlink():
            raise Refused("cleanup refuses a symlink")
        if (target / "postmaster.pid").exists():
            raise Refused("cleanup requires database shutdown")
        if target.exists():
            shutil.rmtree(target)
    for name in ("media-credentials.json", "users.json"):
        (store.directory / name).unlink(missing_ok=True)
    return {
        "outcome": "passed",
        "deleted": ["source", "branch", "postgres", "media", "fixture_credentials"],
        "retained": [
            "owner.json",
            "validate.json",
            "run.json",
            "cleanup.json",
            "receipt.lock",
            "local_logs",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("validate", "run", "cleanup"))
    parser.add_argument("manifest_base64")
    args = parser.parse_args()
    raw = json.loads(base64.b64decode(args.manifest_base64, validate=True))
    manifest = Manifest.parse(raw)
    if (
        hashlib.sha256(Path(sys.argv[0]).read_bytes()).hexdigest()
        != manifest.runner_sha256
    ):
        raise Refused("runner archive does not match the immutable manifest")
    if os.geteuid() == 0:
        command(["ip", "link", "set", "lo", "up"])
        os.setgroups([])
        os.setgid(1000)
        os.setuid(1000)
    assert_network_isolation()
    phase = cast(Phase, args.phase)
    with ReceiptStore(ROOT / "experiments", manifest, phase) as store:
        receipt = store.read()
        if receipt is None:
            if phase != "cleanup" and (store.directory / "cleanup.json").exists():
                raise Refused("cleaned experiment cannot run again")
            atomic_json(
                store.directory / "active.json", {"pid": os.getpid(), "phase": phase}
            )
            try:
                receipt = store.commit(
                    {"validate": validate, "run": run, "cleanup": cleanup}[phase](store)
                )
            finally:
                (store.directory / "active.json").unlink(missing_ok=True)
        sys.stdout.write(json.dumps(receipt, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
