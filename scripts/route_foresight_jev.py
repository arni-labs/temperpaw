#!/usr/bin/env python3
"""Experimental offline selective-review policy; does not change live routing.

A clear score is a routing heuristic, not calibrated correctness. Every missing,
invalid, unknown or defect answer goes to the reasoning reviewer. An invalid
classification response causes the complete packet to be escalated.
"""
import argparse
import json
from pathlib import Path


def route_checks(transitions, categories, classification, clear_threshold=0.80):
    if not 0 <= clear_threshold <= 1:
        raise ValueError('clear_threshold must be between zero and one')
    answers = {} if 'error' in classification else classification.get('response', {}).get('answers', {})
    routed, skipped = {}, {}
    for tid in transitions:
        for kind in categories:
            answer = answers.get(tid + '_' + kind, {})
            probabilities = answer.get('probabilities', {})
            score = probabilities.get('clear')
            valid = (set(probabilities) == {'clear', 'defect', 'unknown'}
                     and all(isinstance(p, (int, float)) and not isinstance(p, bool)
                             and 0 <= p <= 1 for p in probabilities.values())
                     and abs(sum(probabilities.values()) - 1) <= 0.015000001)
            skip = (valid and answer.get('choice') == 'clear'
                    and score == max(probabilities.values())
                    and score >= clear_threshold)
            (skipped if skip else routed).setdefault(tid, []).append(kind)
    return {'route': routed, 'skipped': skipped}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('job', type=Path, help='One review job containing base_message')
    parser.add_argument('classification', type=Path, help='Recorded invoke() result')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    job = json.loads(args.job.read_text())
    body = json.loads(job['base_message'])
    result = route_checks(body['evidence']['transitions'], body['rubric'],
                          json.loads(args.classification.read_text()))
    result['packet_sha256'] = job['packet_sha256']
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
