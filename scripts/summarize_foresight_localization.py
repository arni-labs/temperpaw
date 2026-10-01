#!/usr/bin/env python3
"""Summarize the localization replay without treating selected citations as truth."""
import argparse
from collections import Counter
import json
from pathlib import Path


def summarize(run, original):
    old = {r['path_id']: r for r in original['rows']}
    rows = run['rows']
    assert len({r['path_id'] for r in rows}) == len(rows)
    assert {r['path_id'] for r in rows} == set(old)
    kinds = ('contradiction', 'incentive', 'lag', 'miracle')
    by_kind = {k: Counter() for k in kinds}
    findings, broad_changes = [], []
    for row in rows:
        assert 'error' not in row['classification']
        assert 'error' not in row.get('citations', {})
        answers = row['classification']['response']['answers']
        citations = row.get('citations', {}).get('response', {}).get('answers', {})
        passages = row['classification']['request']['state']['passages']
        expected = {f'{tid}_{kind}' for tid in row['transitions'] for kind in kinds}
        assert set(answers) == expected | {'whole_' + k for k in kinds}
        assert set(citations) == {q for q in expected if answers[q]['choice'] == 'defect'}
        for qid in sorted(expected):
            tid, kind = qid.split('_', 1)
            answer = answers[qid]
            by_kind[kind][answer['choice']] += 1
            if answer['choice'] == 'defect':
                citation = citations[qid]
                by_kind[kind]['citation' if citation['choice'] != 'none' else 'no_citation'] += 1
                assert citation['choice'] == 'none' or citation['choice'] in passages
                findings.append({'path_id': row['path_id'], 'question': qid,
                                 'transition': row['transitions'][tid], 'finding': kind,
                                 'probabilities': answer['probabilities'],
                                 'confidence': answer['confidence'], 'citation': citation,
                                 'passage': passages.get(citation['choice'])})
        for kind in kinds:
            previous = old[row['path_id']]['jev'][kind]['choice']
            current = answers['whole_' + kind]['choice']
            if previous != current:
                broad_changes.append({'path_id': row['path_id'], 'kind': kind,
                                      'before': previous, 'after': current})
            local = any(answers[q]['choice'] == 'defect' for q in expected if q.endswith('_' + kind))
            cited = any(c['choice'] != 'none' for q, c in citations.items() if q.endswith('_' + kind))
            by_kind[kind]['paths_with_local_defect'] += local
            by_kind[kind]['paths_with_cited_defect'] += cited
            if previous != old[row['path_id']]['reasoner'][kind]['choice']:
                by_kind[kind]['original_disagreements'] += 1
                by_kind[kind]['disagreements_localized'] += local
                by_kind[kind]['disagreements_cited'] += cited
    return {'paths': len(rows), 'transitions': sum(len(r['transitions']) for r in rows),
            'external_edges_not_individually_tested': sum(len(r['external_edges_not_individually_tested']) for r in rows),
            'by_kind': by_kind, 'broad_unchanged': 4 * len(rows) - len(broad_changes),
            'broad_changes': broad_changes, 'findings': findings,
            'limits': ['Citation selection is not independent evidence validation.',
                       'No citation does not prove absence of a defect; some require multiple passages.',
                       'Only edges between required nodes were individually checked.',
                       'Same evidence but indexed presentation and narrower questions; not the original paired test.']}


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('run', type=Path)
    ap.add_argument('original', type=Path)
    args = ap.parse_args()
    print(json.dumps(summarize(json.loads(args.run.read_text()), json.loads(args.original.read_text())), indent=2))
