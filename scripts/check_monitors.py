#!/usr/bin/env python3
"""Checks on dd-monitors/temperpaw-monitors.json that need no Datadog call.

A grouped metric monitor (`... by {...}`) keeps each group's last state when that group stops reporting, and
renotifies it for as long as the monitor exists: on 28 Oct 2026 "[Temper] WASM Host HTTP Duration p95
Regression" renotified every 2 hours for 8 hours after its last 5xx call. So a grouped monitor resolves a group
that goes quiet, in one of Datadog's two ways: options.timeout_h (that many hours without data), or
options.on_missing_data "resolve" (Datadog refuses both together, and refuses "resolve" on default_zero queries).

The exception is a monitor whose silence is itself the alarm (a no-data or heartbeat monitor). Resolving on silence
would hide the very thing it watches, so such a monitor carries the tag `silence:alarm` instead, and sets neither.

CI runs this on every pull request, and deploy_monitors.py runs it before any Datadog call. Standard library only.
Usage: check_monitors.py [monitors.json]
"""

import json
import re
import sys
from pathlib import Path

SILENCE_TAG = "silence:alarm"
GROUPED = re.compile(r"\bby\s*\{")
MONITORS = Path(__file__).resolve().parent.parent / "dd-monitors" / "temperpaw-monitors.json"


def resolves_quiet_groups(options: dict) -> bool:
    timeout_h = options.get("timeout_h")
    has_timeout = isinstance(timeout_h, int) and not isinstance(timeout_h, bool) and timeout_h > 0
    return has_timeout or options.get("on_missing_data") == "resolve"


def problems(monitors: list[dict]) -> list[str]:
    """Why each monitor that breaks the rule above is refused."""
    found = []
    for monitor in monitors:
        name = monitor.get("name", "<unnamed>")
        options = monitor.get("options") or {}
        silence = SILENCE_TAG in (monitor.get("tags") or [])
        if silence and ("timeout_h" in options or options.get("on_missing_data") == "resolve"):
            found.append(f"{name}: tagged {SILENCE_TAG}, so it must not resolve quiet groups (it would hide the silence it watches)")
        elif silence or not GROUPED.search(monitor.get("query", "")):
            continue
        elif not resolves_quiet_groups(options):
            found.append(
                f"{name}: a grouped monitor needs options.timeout_h (a positive number of hours) or on_missing_data "
                f'"resolve", so a group that stops reporting resolves; tag it {SILENCE_TAG} instead only when its '
                "silence is the alarm"
            )
    return found


def main() -> int:
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else MONITORS
    found = problems(json.loads(path.read_text()))
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        print(f"{len(found)} monitor(s) refused in {path}", file=sys.stderr)
        return 1
    print(f"ok: {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
