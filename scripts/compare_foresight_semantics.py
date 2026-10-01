#!/usr/bin/env python3
"""Summarize exported shadow records; optional labels are judgments, not outcomes.

Usage: compare_foresight_semantics.py evaluations.json [--labels heldout.json]
Labels: {input_sha256: {"source": "human", "split": "heldout",
                       "answers": {"evidence": "clear", ...}}}
Failed and skipped rows remain in coverage denominators. Repeated input hashes
are excluded from labeled quality metrics but retained in operational metrics.
"""
import argparse
from collections import Counter
import json
import math
from pathlib import Path


def percentile(values, fraction):
    if not values:
        return None
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize(rows, labels):
    statuses = Counter()
    errors = Counter()
    models = Counter()
    durations = []
    tokens = 0
    judgments = Counter()
    evaluated_hashes = set()
    quality = {k: {"n": 0, "correct": 0, "brier_sum": 0.0}
               for k in ("contradiction", "incentive", "lag", "miracle",
                         "evidence", "prerequisite", "timing")}
    for row in rows:
        fields = row.get("fields", row)
        status = row.get("status", fields.get("Status"))
        statuses[status or "Unknown"] += 1
        if status != "Recorded":
            if fields.get("error_message"):
                errors[fields["error_message"]] += 1
            continue
        record = json.loads(fields["result_json"])
        response = record["response"]
        models[response["model"]] += 1
        duration = record.get("provider_elapsed_ms")
        if isinstance(duration, (int, float)) and duration >= 0:
            durations.append(duration)
        tokens += response["usage"]["input_tokens"]
        for key, answer in response["answers"].items():
            judgments[f"{key}:{answer['choice']}"] += 1
        digest = record["input_sha256"]
        label = labels.get(digest)
        if not label or digest in evaluated_hashes:
            continue
        if label.get("source") != "human" or label.get("split") != "heldout":
            raise ValueError("Quality comparison requires human held-out labels")
        evaluated_hashes.add(digest)
        for key, expected in label["answers"].items():
            if key not in quality or expected not in ("clear", "defect", "unknown"):
                raise ValueError("Unknown question or label")
            answer = response["answers"][key]
            q = quality[key]
            q["n"] += 1
            q["correct"] += answer["choice"] == expected
            mass = sum(answer["probabilities"].values())
            q["brier_sum"] += sum((p / mass - float(option == expected)) ** 2
                                   for option, p in answer["probabilities"].items())
    return {
        "records": len(rows), "statuses": dict(statuses), "errors": dict(errors),
        "recorded_fraction": statuses["Recorded"] / len(rows) if rows else None,
        "models": dict(models), "input_tokens": tokens,
        "provider_latency_samples": len(durations),
        "provider_p50_ms": percentile(durations, 0.5),
        "provider_p95_ms": percentile(durations, 0.95),
        "judgments": dict(judgments),
        "heldout_semantic_quality": {
            key: {"n": q["n"],
                  "accuracy": q["correct"] / q["n"] if q["n"] else None,
                  "multiclass_brier": q["brier_sum"] / q["n"] if q["n"] else None}
            for key, q in quality.items()},
        "limits": ["Shadow mode retains every existing critic call.",
                   "Provider latency is not whole-pipeline elapsed time.",
                   "Semantic labels do not measure future forecast accuracy.",
                   "Brier scores normalize rounded probabilities; raw records remain unchanged."]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evaluations", type=Path)
    parser.add_argument("--labels", type=Path)
    args = parser.parse_args()
    data = json.loads(args.evaluations.read_text())
    rows = data if isinstance(data, list) else data["value"]
    labels = json.loads(args.labels.read_text()) if args.labels else {}
    print(json.dumps(summarize(rows, labels), indent=2))
