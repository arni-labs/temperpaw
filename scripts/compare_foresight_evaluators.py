#!/usr/bin/env python3
"""Compare paired frozen-packet runs, reporting agreement rather than accuracy.

Inputs: fair-packets.json, live WASM replay output, and Foresight Session exports.
No inference, secret access, threshold tuning or production writes occur here.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import statistics

QUESTIONS = ('contradiction', 'incentive', 'lag', 'miracle')
CHOICES = {'clear', 'defect', 'unknown'}


def indexed(rows, key):
    result = {row[key]: row for row in rows}
    if len(result) != len(rows):
        raise ValueError(f'duplicate {key}')
    return result


def compare(packets, jev_run, sessions, price=None):
    packets = indexed(packets, 'path_id')
    jev = indexed(jev_run['rows'], 'path_id')
    reasoner = indexed(sessions, 'path_id')
    if packets.keys() != jev.keys() or packets.keys() != reasoner.keys():
        raise ValueError('unpaired/missing cases; do not silently drop failures')
    rows, latencies, completion_bounds = [], [], []
    tokens = 0
    for path, packet in packets.items():
        digest = hashlib.sha256(packet['packet_json'].encode()).hexdigest()
        if digest != packet['packet_sha256']:
            raise ValueError('frozen packet hash mismatch')
        j, session = jev[path], reasoner[path]
        if session['result']['status'] != 'Completed':
            raise ValueError(f'reasoner failed for {path}')
        result = session['result']['fields']['result'].strip()
        if result.startswith('```json') and result.endswith('```'):
            result = result[7:-3].strip()
        g = json.loads(result)
        record = j['record']
        if any(value != digest for value in [j['packet_sha256'], record['packet_sha256'],
                                             session['packet_sha256'], g['packet_sha256']]):
            raise ValueError('evaluators used different packet hashes')
        frozen = json.loads(packet['packet_json'])
        if frozen != packet['packet']:
            raise ValueError('packet object differs from frozen serialization')
        for key in ('state', 'questions'):
            if record['request'][key] != frozen[key]:
                raise ValueError(f'Jev request changed frozen {key}')
        answers = record['response']['answers']
        if set(answers) != set(QUESTIONS) or set(g['answers']) != set(QUESTIONS):
            raise ValueError('question set differs')
        for key in QUESTIONS:
            if answers[key]['choice'] not in CHOICES or g['answers'][key]['choice'] not in CHOICES:
                raise ValueError('invalid judgment')
        rows.append({'path_id': path, 'packet_sha256': digest,
                     'reasoner': g['answers'], 'jev': answers})
        latencies.append(j['seconds'])
        tokens += record['response']['usage']['input_tokens']
        if 'observed_complete_ms' in session:
            completion_bounds.append(((session['last_pending_ms'] - session['started_ms']) / 1000,
                                      (session['observed_complete_ms'] - session['started_ms']) / 1000))
    counts = {key: {
        'agreement': sum(row['reasoner'][key]['choice'] == row['jev'][key]['choice'] for row in rows),
        'total': len(rows),
        'reasoner_counts': dict(Counter(row['reasoner'][key]['choice'] for row in rows)),
        'jev_counts': dict(Counter(row['jev'][key]['choice'] for row in rows))} for key in QUESTIONS}
    return {
        'paired_paths': len(rows), 'paired_judgments': len(rows) * len(QUESTIONS),
        'agreements': sum(c['agreement'] for c in counts.values()),
        'by_question': counts,
        'jev_invocation_median_seconds': statistics.median(latencies) if latencies else None,
        'jev_invocation_p95_seconds': sorted(latencies)[math.ceil(.95 * len(latencies)) - 1] if latencies else None,
        'jev_batch_seconds': jev_run['batch_seconds'],
        'jev_input_tokens': tokens,
        'jev_estimated_input_cost_usd': tokens * price / 1e6 if price is not None else None,
        'reasoner_timed_sessions': len(completion_bounds),
        'reasoner_session_median_lower_seconds': statistics.median(b[0] for b in completion_bounds) if completion_bounds else None,
        'reasoner_session_median_upper_seconds': statistics.median(b[1] for b in completion_bounds) if completion_bounds else None,
        'limits': ['Agreement is not accuracy; no independent truth labels.',
                   'Reasoner includes explanations and session orchestration; Jev invocation latency is not an equivalent clock.',
                   'Session token counters report 1/1, so reasoning-model usage and cost are unavailable.',
                   'Common upstream paths are frozen; this is not a new research-to-registration pipeline run.',
                   'One incomplete source path is outside the 22 paired cases.'],
        'rows': rows}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('packets', type=Path)
    parser.add_argument('jev', type=Path)
    parser.add_argument('reasoner', type=Path)
    parser.add_argument('--jev-input-price-per-million', type=float)
    args = parser.parse_args()
    print(json.dumps(compare(json.loads(args.packets.read_text()), json.loads(args.jev.read_text()),
                             json.loads(args.reasoner.read_text()), args.jev_input_price_per_million), indent=2))
