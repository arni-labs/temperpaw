#!/usr/bin/env python3
"""Deploy TemperPaw self-monitoring Datadog monitors.

Reads dd-monitors/temperpaw-monitors.json and creates or updates each monitor
via the Datadog REST API. Idempotent: finds existing monitors by name.

With --reconcile, also deletes TemperPaw-owned monitors that are NOT in the JSON
file. A monitor is TemperPaw's when it carries the team:temperpaw tag or has a
name from the file; nothing else marks ownership. Words in a monitor's text do
not: Katagami's own monitors name the Railway service in their runbooks, and
matching that text would delete them. This is the source-of-truth guarantee
declared in ADR-0052: file-first, Datadog state reconciles to match.

Requires DD_API_KEY and DD_APP_KEY in env (or .env file).
"""

import argparse
import json
import os
import sys
import time
from pathlib import Path

try:
    import requests
except ImportError:
    sys.exit("requests is required: pip install requests")

TEAM_TAG = "team:temperpaw"


# Datadog treats these two as one kind of monitor: a PUT with either keeps the live type, and saving a monitor in
# the Datadog UI turns "metric alert" into "query alert". Recreating a monitor over that difference would only
# give it a new id and drop its history.
METRIC_MONITOR_TYPES = {"metric alert", "query alert"}


def same_monitor_type(live_type: str | None, file_type: str | None) -> bool:
    return live_type == file_type or {live_type, file_type} <= METRIC_MONITOR_TYPES


def load_env():
    """Best-effort .env loading."""
    env_path = Path(__file__).resolve().parent.parent / ".env"
    if env_path.exists():
        for line in env_path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, _, value = line.partition("=")
            os.environ.setdefault(key.strip(), value.strip())


def is_temperpaw_owned_monitor(monitor: dict, desired_names: set[str]) -> bool:
    tags = set(monitor.get("tags") or [])
    return monitor.get("name") in desired_names or TEAM_TAG in tags


def monitor_preference_score(monitor: dict, desired_names: set[str]) -> tuple[int, int]:
    """Prefer the team-tagged monitor when copies share a name."""
    tags = set(monitor.get("tags") or [])
    return (
        int(TEAM_TAG in tags),
        int(monitor.get("name") in desired_names),
    )


def index_existing_monitors(
    existing_monitors: list[dict],
    desired_names: set[str],
) -> tuple[dict[str, dict], list[dict]]:
    existing_by_name: dict[str, dict] = {}
    duplicates: list[dict] = []

    for monitor in existing_monitors:
        name = monitor["name"]
        current = existing_by_name.get(name)
        if current is None:
            existing_by_name[name] = monitor
            continue

        if monitor_preference_score(monitor, desired_names) > monitor_preference_score(
            current,
            desired_names,
        ):
            duplicates.append(current)
            existing_by_name[name] = monitor
        else:
            duplicates.append(monitor)

    return existing_by_name, duplicates


def raise_for_status(resp: requests.Response, action: str):
    if resp.ok:
        return
    body = resp.text.strip()
    detail = f": {body}" if body else ""
    raise requests.HTTPError(
        f"{action} failed with {resp.status_code} {resp.reason}{detail}",
        response=resp,
    )


def datadog_request(
    method: str,
    url: str,
    headers: dict,
    *,
    action: str,
    raise_on_error: bool = True,
    **kwargs,
) -> requests.Response:
    """Call Datadog with bounded retry/backoff for rate limits and 5xx."""
    resp = None
    for attempt in range(6):
        resp = requests.request(method, url, headers=headers, timeout=30, **kwargs)
        if resp.status_code != 429 and resp.status_code < 500:
            break
        retry_after = resp.headers.get("Retry-After")
        if retry_after and retry_after.isdigit():
            sleep_seconds = int(retry_after)
        else:
            sleep_seconds = min(2**attempt, 30)
        time.sleep(sleep_seconds)

    assert resp is not None
    if raise_on_error:
        raise_for_status(resp, action)
    return resp


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--reconcile",
        action="store_true",
        help="Delete TemperPaw-owned monitors that are not in the JSON file.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would happen without making changes.",
    )
    args = parser.parse_args()

    load_env()

    api_key = os.environ.get("DD_API_KEY", "")
    app_key = os.environ.get("DD_APP_KEY", "")
    site = os.environ.get("DD_SITE", "datadoghq.com")

    if not api_key or not app_key:
        sys.exit("DD_API_KEY and DD_APP_KEY must be set")

    monitors_path = (
        Path(__file__).resolve().parent.parent
        / "dd-monitors"
        / "temperpaw-monitors.json"
    )
    monitors = json.loads(monitors_path.read_text())
    desired_names = {m["name"] for m in monitors}

    base_url = f"https://api.{site}/api/v1"
    headers = {
        "DD-API-KEY": api_key,
        "DD-APPLICATION-KEY": app_key,
        "Content-Type": "application/json",
    }

    validation_errors = []
    for monitor in monitors:
        resp = datadog_request(
            "POST",
            f"{base_url}/monitor/validate",
            headers=headers,
            action=f"Validate monitor {monitor.get('name', '<unnamed>')}",
            raise_on_error=False,
            json=monitor,
        )
        if resp.status_code >= 400:
            validation_errors.append(
                f"{monitor.get('name', '<unnamed>')}: {resp.status_code} {resp.text}"
            )

    if validation_errors:
        joined = "\n\n".join(validation_errors)
        sys.exit(f"Datadog monitor validation failed:\n\n{joined}")

    resp = datadog_request(
        "GET",
        f"{base_url}/monitor",
        headers=headers,
        action="List monitors",
    )
    existing_monitors = [
        m for m in resp.json() if is_temperpaw_owned_monitor(m, desired_names)
    ]
    existing_by_name, duplicate_existing_monitors = index_existing_monitors(
        existing_monitors,
        desired_names,
    )

    for monitor in monitors:
        name = monitor["name"]
        if name in existing_by_name:
            existing = existing_by_name[name]
            monitor_id = existing["id"]
            if not same_monitor_type(existing.get("type"), monitor.get("type")):
                if args.dry_run:
                    print(
                        f"[dry-run] Would recreate: {name} "
                        f"(id={monitor_id}, {existing.get('type')} -> {monitor.get('type')})"
                    )
                    continue
                datadog_request(
                    "DELETE",
                    f"{base_url}/monitor/{monitor_id}",
                    headers=headers,
                    action=f"Delete monitor {name} ({monitor_id})",
                )
                resp = datadog_request(
                    "POST",
                    f"{base_url}/monitor",
                    headers=headers,
                    action=f"Create monitor {name}",
                    json=monitor,
                )
                monitor_id = resp.json().get("id", "unknown")
                print(
                    f"Recreated: {name} "
                    f"({existing.get('type')} -> {monitor.get('type')}, id={monitor_id})"
                )
                continue
            if args.dry_run:
                print(f"[dry-run] Would update: {name} (id={monitor_id})")
                continue
            datadog_request(
                "PUT",
                f"{base_url}/monitor/{monitor_id}",
                headers=headers,
                action=f"Update monitor {name} ({monitor_id})",
                json=monitor,
            )
            print(f"Updated: {name} (id={monitor_id})")
        else:
            if args.dry_run:
                print(f"[dry-run] Would create: {name}")
                continue
            resp = datadog_request(
                "POST",
                f"{base_url}/monitor",
                headers=headers,
                action=f"Create monitor {name}",
                json=monitor,
            )
            monitor_id = resp.json().get("id", "unknown")
            print(f"Created: {name} (id={monitor_id})")

    if args.reconcile:
        orphans = {
            (m["name"], m["id"])
            for m in duplicate_existing_monitors
        }
        orphans.update(
            (m["name"], m["id"])
            for m in existing_monitors
            if m.get("name") not in desired_names
        )
        orphans = sorted(orphans)
        if not orphans:
            print("No orphan monitors to reconcile.")
            return
        print(f"\nReconcile: {len(orphans)} orphan monitor(s) to delete:")
        for name, monitor_id in orphans:
            if args.dry_run:
                print(f"  [dry-run] Would delete: {name} (id={monitor_id})")
                continue
            datadog_request(
                "DELETE",
                f"{base_url}/monitor/{monitor_id}",
                headers=headers,
                action=f"Delete orphan monitor {name} ({monitor_id})",
            )
            print(f"  Deleted: {name} (id={monitor_id})")


if __name__ == "__main__":
    main()
