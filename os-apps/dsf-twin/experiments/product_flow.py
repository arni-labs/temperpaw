"""Real DSF HTTP product checks inside the experiment's network namespace."""

from __future__ import annotations

import json
import secrets
import subprocess
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING
from uuid import uuid4

if TYPE_CHECKING:
    from runner import ReceiptStore

BASE = "http://127.0.0.1:53500"


def request(
    method: str,
    path: str,
    expected: int = 200,
    body: object = None,
    key: str | None = None,
    extra_headers: dict[str, str] | None = None,
) -> dict:
    headers = {"Content-Type": "application/json"}
    if key:
        headers["X-API-Key"] = key
    headers.update(extra_headers or {})
    message = urllib.request.Request(
        BASE + path,
        method=method,
        headers=headers,
        data=None if body is None else json.dumps(body).encode(),
    )
    try:
        response = urllib.request.urlopen(message, timeout=30)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        assert response.status == expected, (method, path, response.status, expected)
        return json.load(response)


@contextmanager
def api(source: Path, directory: Path, env: dict[str, str]) -> Iterator[None]:
    from runner import ROOT, Refused

    with (directory / "api.log").open("ab") as log:
        process = subprocess.Popen(
            [
                str(ROOT / "tools/venv/bin/python"),
                "-m",
                "uvicorn",
                "main:app",
                "--host",
                "127.0.0.1",
                "--port",
                "53500",
            ],
            cwd=source / "platform/backend",
            env=env,
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 40
            while True:
                try:
                    request("GET", "/health")
                    break
                except (OSError, urllib.error.URLError):
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise Refused(
                            "isolated DSF server did not become healthy"
                        ) from None
                    time.sleep(0.2)
            yield
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def verify(store: ReceiptStore, source: Path) -> dict[str, object]:
    from runner import ROOT, command, psql, safe_environment

    database = store.manifest.database_id
    # A lost result can restart this bounded test from its own clean schema. No outside write exists.
    psql(
        store,
        "DROP SCHEMA public CASCADE; CREATE SCHEMA public; CREATE EXTENSION vector",
        database,
    )
    env = {
        **safe_environment(store.directory),
        "DATABASE_URL": f"postgresql://experiment@127.0.0.1:55432/{database}",
        "TESTING": "true",
        "DSF_TEST_MODE_ENABLED": "true",
        "ACTION_QUEUE_WORKER_ENABLED": "false",
        "DD_TRACE_ENABLED": "false",
        "DD_INSTRUMENTATION_TELEMETRY_ENABLED": "false",
        "ENVIRONMENT": "experiment",
        "RAILWAY_GIT_COMMIT_SHA": store.manifest.source_revision,
        "GIT_SHA": store.manifest.source_revision,
        "CORS_ORIGINS": store.manifest.cors_origin,
        "GUIDANCE_TOKEN_SECRET": secrets.token_hex(32),
        "R2_BUCKET_NAME": store.manifest.media_bucket,
    }
    command(
        [ROOT / "tools/venv/bin/python", "-m", "alembic", "upgrade", "head"],
        cwd=source / "platform/backend",
        env=env,
    )
    credentials: dict[str, dict[str, str]] = {}
    with api(source, store.directory, env):
        for role in ("author", "reviewer1", "reviewer2"):
            result = request(
                "POST",
                "/api/auth/agent",
                body={"name": "Experiment " + role, "username": role},
            )
            credentials[role] = {
                "id": result["agent"]["id"],
                "key": result["api_key"]["key"],
            }
    env.update(
        {
            "ADMIN_API_KEY": credentials["author"]["key"],
            "ADMIN_API_KEYS": credentials["author"]["key"],
        }
    )
    with api(source, store.directory, env):
        return product_checks(store, credentials)


def product_checks(
    store: ReceiptStore, credentials: dict[str, dict[str, str]]
) -> dict[str, object]:
    checks: list[dict[str, object]] = []

    def call(
        method: str,
        path: str,
        expected: int = 200,
        role: str | None = None,
        body: object = None,
        extra_headers: dict[str, str] | None = None,
    ) -> dict:
        result = request(
            method,
            path,
            expected,
            body,
            credentials[role]["key"] if role else None,
            extra_headers,
        )
        checks.append({"method": method, "path": path, "status": expected})
        return result

    call(
        "POST",
        "/api/admin/guidance/story-writing",
        role="author",
        body={
            "version": "arn467-experiment",
            "rules": [
                {
                    "id": "physical-constraint",
                    "severity": "error",
                    "text": "Show a concrete resource constraint and the causal effect of the character decision.",
                }
            ],
            "examples": [],
        },
    )
    for origin, expected in (
        (store.manifest.cors_origin, 200),
        ("https://outside.invalid", 400),
    ):
        req = urllib.request.Request(
            BASE + "/api/worlds",
            method="OPTIONS",
            headers={"Origin": origin, "Access-Control-Request-Method": "GET"},
        )
        try:
            response = urllib.request.urlopen(req, timeout=5)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            assert response.status == expected
            assert response.headers.get("access-control-allow-origin") == (
                origin if expected == 200 else None
            )
            checks.append({"method": "OPTIONS", "origin": origin, "status": expected})

    health = call("GET", "/health")
    assert health["status"] == "healthy" and health["schema"]["is_current"]
    assert health["git_sha"] == store.manifest.source_revision
    call("GET", "/api/operations/snapshot", 401)
    call("GET", "/api/operations/snapshot", 403, "reviewer1")
    first = call("GET", "/api/operations/snapshot", role="author")
    second = call("GET", "/api/operations/snapshot", role="author")
    assert first["participants"] == second["participants"]
    assert first["revision"] == health["git_sha"]
    assert first["schema"]["is_current"]
    ids: list[str] = []
    path: str | None = "/api/operations/snapshot?participant_limit=1"
    while path:
        page = call("GET", path, role="author")["participants"]
        assert len(page["items"]) <= 1
        ids.extend(item["id"] for item in page["items"])
        assert len(ids) < 100
        path = (
            "/api/operations/snapshot?participant_limit=1&participant_after="
            + page["next_cursor"]
            if page["next_cursor"]
            else None
        )
    assert len(ids) == len(set(ids)) == first["participant_summary"]["total"]
    for method in ("POST", "PATCH", "PUT", "DELETE"):
        call(method, "/api/operations/snapshot", 405, "author", {})
    call("GET", "/api/operations/snapshot?job_limit=101", 422, "author")
    call("GET", "/api/operations/snapshot?participant_limit=201", 422, "author")
    operation = {"operation_id": str(uuid4()), "generation_ids": [str(uuid4())]}
    call("POST", "/api/media/retry-stuck", 401, body=operation)
    call("POST", "/api/media/retry-stuck", 403, "reviewer1", operation)
    result = call("POST", "/api/media/retry-stuck", role="author", body=operation)
    assert result["queued"] == 0 and result["generations"][0]["outcome"] == "missing"
    assert call("POST", "/api/media/retry-stuck", role="author", body=operation) == {
        **result,
        "replayed": True,
    }
    call("POST", "/api/media/process-pending", 409, "author", operation)

    suffix = str(uuid4())[:8]
    proposal = {
        "name": "Independent Water Dispatch " + suffix,
        "premise": "A coastal city must allocate desalinated water while a heat wave lowers membrane output and strains a fixed electricity budget.",
        "scientific_basis": "Reverse osmosis requires membrane pressure and electrical pumping power. Fouling reduces throughput, so measured output and electricity limits constrain every district allocation.",
        "image_prompt": "A realistic coastal desalination plant with membrane housings, flow meters and technicians at dawn.",
        "year_setting": 2042,
        "causal_chain": [
            {
                "year": 2028,
                "event": "Utilities measure membrane pressure and flow.",
                "reasoning": "Existing sensors support repeated calibration.",
                "consequence": "Operators compare output against electricity use.",
            },
            {
                "year": 2034,
                "event": "District contracts incorporate measured pump power.",
                "reasoning": "Demand forecasts exceed available reserve capacity.",
                "consequence": "Water dispatch must include equipment limits.",
            },
            {
                "year": 2042,
                "event": "A heat wave coincides with delayed membrane replacements.",
                "reasoning": "Higher intake temperatures and fouling lower output.",
                "consequence": "Districts negotiate rationing under a fixed pumping budget.",
            },
        ],
    }
    created = call("POST", "/api/proposals", role="author", body=proposal)
    proposal_id = created["id"]
    call(
        "POST",
        f"/api/proposals/{proposal_id}/submit?force=true",
        role="author",
        body={},
    )
    feedback: list[tuple[str, str]] = []
    for role in ("reviewer1", "reviewer2"):
        response = call(
            "POST",
            f"/api/review/proposal/{proposal_id}/feedback",
            role=role,
            body={
                "feedback_items": [
                    {
                        "category": "scientific_issue",
                        "severity": "important",
                        "description": "State the measured membrane output and the pump power limit so district allocations can be checked against physical capacity.",
                    }
                ]
            },
        )
        feedback.append((role, response["feedback_items"][0]["id"]))
    for _, item in feedback:
        call(
            "POST",
            f"/api/review/feedback-item/{item}/respond",
            role="author",
            body={
                "response_text": "The revised account requires every allocation interval to stay within measured plant throughput and the electrical pumping ceiling."
            },
        )
    call(
        "POST",
        f"/api/proposals/{proposal_id}/revise",
        role="author",
        body={
            "scientific_basis": str(proposal["scientific_basis"])
            + " Every interval is checked against measured throughput and the current electrical ceiling."
        },
    )
    for role, item in feedback:
        resolved = call(
            "POST", f"/api/review/feedback-item/{item}/resolve", role=role, body={}
        )
    world_id = resolved["graduation"]["world_id"]
    assert (
        call("GET", f"/api/proposals/{proposal_id}", role="author")["proposal"][
            "status"
        ]
        == "approved"
    )
    call("GET", f"/api/worlds/{world_id}")

    story = {
        "world_id": world_id,
        "title": "The Pumping Ceiling " + suffix,
        "perspective": "first_person_agent",
        "content": "I checked the plant at dawn. The district requested more water, but output had fallen, so I changed the allocation.",
        "image_prompt": "A realistic desalination control room with pressure gauges and a coastal view at dawn.",
    }
    blocked = call("POST", "/api/stories", 428, "author", story)
    revised = "At dawn the pressure gauge held steady but the outlet meter lost another cubic metre per minute. I opened the intake-temperature log. The warm seawater had reduced the margin we used for membrane cleaning. District Nine wanted another hour of cooling water. Granting it would force the pumps beyond their electrical ceiling or postpone the rinse until tomorrow. I sent both measurements to the district operator. She closed two greenhouse loops and returned a smaller request for the clinic alone. I approved that amount, watched the pump load settle below its limit, and scheduled the rinse while the unused greenhouse pipes cooled."
    ack = call(
        "POST",
        "/api/stories/acknowledge",
        role="author",
        body={
            "submission_hash": blocked["detail"]["submission_hash"],
            "story_content": revised,
            "compliance": {
                "physical-constraint": "Measured membrane output and pump power limit the allocation, causing the operator to preserve clinic cooling and close greenhouse loops."
            },
        },
    )
    story["content"] = revised
    created_story = call(
        "POST",
        "/api/stories",
        role="author",
        body=story,
        extra_headers={"X-Guidance-Token": ack["token"]},
    )
    story_id = created_story["story"]["id"]
    readback = call("GET", f"/api/stories/{story_id}")
    assert readback["story"]["content"] == revised
    call(
        "POST", "/api/stories", 401, "author", story, {"X-Guidance-Token": ack["token"]}
    )
    return {
        "outcome": "passed",
        "checks": checks,
        "check_count": len(checks),
        "cors_origin": store.manifest.cors_origin,
        "proposal_id": proposal_id,
        "world_id": world_id,
        "story_id": story_id,
        "story_content": revised,
        "paid_provider_calls": 0,
        "external_calls": [],
    }
