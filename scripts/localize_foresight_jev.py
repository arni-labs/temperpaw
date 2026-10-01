#!/usr/bin/env python3
"""Explicit offline replay experiment; never mutates Foresight entities.

Keep the complete frozen evidence, index its verbatim passages, evaluate actual
intra-path dependency edges, then request citations for defects. Citation choices
are leads, not validated explanations. No credentials are written to outputs.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import time
import urllib.error
import urllib.request


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def prepare(packet):
    raw = packet['packet_json']
    assert digest(raw) == packet['packet_sha256']
    frozen = json.loads(raw)
    state = dict(frozen['state'])
    passages = {}
    for document in ('repair', 'endpoint_bundle', 'observed_graph'):
        body = state.pop(document)
        chunks = re.findall(r'.+?(?:\n\s*\n|\Z)', body, re.S)
        assert ''.join(chunks) == body
        offset = 0
        for chunk in chunks:
            name = f'p{len(passages)+1:03d}'
            passages[name] = {'document': document, 'start': offset,
                              'end': offset + len(chunk), 'text': chunk}
            offset += len(chunk)
    assert len(passages) < 255, 'do not silently shortlist citation candidates'
    state['passages'] = passages
    nodes = {n['id']: n for n in state['required_nodes']}
    transitions, external = {}, []
    for target in nodes.values():
        for source_id in json.loads(target['edges']):
            if source_id not in nodes:
                external.append({'source': source_id, 'target': target['id']})
                continue
            source = nodes[source_id]
            tid = f't{len(transitions)+1:02d}'
            transitions[tid] = {
                'source_id': source_id, 'target_id': target['id'],
                'source': source['statement'], 'source_date': source['resolve_by'],
                'target': target['statement'], 'target_date': target['resolve_by']}
    assert transitions
    state['transitions'] = transitions
    questions = {'whole_' + k: q for k, q in frozen['questions'].items()}
    for tid, transition in transitions.items():
        for kind, original in frozen['questions'].items():
            questions[tid + '_' + kind] = {
                **original,
                'instructions': (
                    f'Evaluate ONLY the dependency {tid}: {transition["source_id"]} '
                    f'({transition["source_date"]}) -> {transition["target_id"]} '
                    f'({transition["target_date"]}), described in state.transitions.{tid}. '
                    'Use all supplied passages and the rest of the graph as context. '
                    'Do not attribute a defect elsewhere in the path to this dependency. '
                    'A dependency can have multiple defect types. '
                    + original['instructions'])}
    return {'path_id': packet['path_id'], 'packet_sha256': packet['packet_sha256'],
            'transitions': transitions, 'external_edges_not_individually_tested': external,
            'request': {'model': 'jev-1.13.0', 'state': state, 'questions': questions}}


def invoke(request, key):
    data = json.dumps(request, ensure_ascii=False, separators=(',', ':')).encode()
    start = time.perf_counter()
    result = {'request': request, 'request_sha256': hashlib.sha256(data).hexdigest()}
    try:
        req = urllib.request.Request('https://api.typesafe.ai/v1/systemone', data=data,
                                     headers={'Authorization': 'Bearer ' + key,
                                              'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=90) as response:
            out = json.load(response)
        result['response'] = out
        assert out['model'] == request['model']
        assert set(out['answers']) == set(request['questions'])
        for qid, answer in out['answers'].items():
            options = request['questions'][qid]['criteria']
            assert answer['choice'] in options
            assert set(answer['probabilities']) == set(options)
            probs = answer['probabilities']
            assert all(0 <= p <= 1 for p in probs.values())
            assert abs(sum(probs.values()) - 1) <= len(probs) * .005 + 1e-9
            assert probs[answer['choice']] == max(probs.values())
            assert 0 <= answer['confidence'] <= 1
    except urllib.error.HTTPError as exc:
        result['error'] = 'HTTP_' + str(exc.code)
        # Preserve only the provider's structured error type, never echoed data.
        try:
            result['provider_error_type'] = json.load(exc).get('detail', {}).get('error_type')
        except (ValueError, AttributeError):
            pass
    except Exception as exc:
        result['error'] = type(exc).__name__
    result['seconds'] = time.perf_counter() - start
    return result


def citation_request(prepared, classification):
    questions = {}
    for qid, answer in classification['response']['answers'].items():
        if qid.startswith('whole_') or answer['choice'] != 'defect':
            continue
        tid, kind = qid.split('_', 1)
        questions[qid] = {
            'type': 'choice',
            'instructions': (
                f'A prior pass flagged a {kind} defect on transition {tid}, '
                f'{prepared["transitions"][tid]["source_id"]} -> '
                f'{prepared["transitions"][tid]["target_id"]}. '
                'That prior answer may be wrong. Select the passage that most directly '
                'supports this specific defect on this specific dependency, not merely '
                'one that mentions the same topic or repeats an unsupported judgment. '
                'An authored future claim is not an observation. If no single passage '
                'adequately supports it, select none. Use only the supplied evidence.'),
            'criteria': {
                **{pid: f'The exact passage state.passages.{pid} directly supports this defect.'
                   for pid in prepared['request']['state']['passages']},
                'none': 'No single supplied passage adequately supports this specific finding.'}}
    return {'model': 'jev-1.13.0', 'state': prepared['request']['state'], 'questions': questions}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('packets', type=Path)
    ap.add_argument('output', type=Path)
    ap.add_argument('--key-file', type=Path, required=True)
    ap.add_argument('--limit', type=int)
    ap.add_argument('--resume', action='store_true')
    args = ap.parse_args()
    key = args.key_file.read_text().strip()
    packets = json.loads(args.packets.read_text())
    if args.limit:
        packets = packets[:args.limit]
    prepared = [prepare(p) for p in packets]
    previous = {}
    if args.resume and args.output.exists():
        previous = {r['path_id']: r for r in json.loads(args.output.read_text())['rows']}
    start = time.perf_counter()
    rows = []
    def run(p):
        row = previous.get(p['path_id']) or {k: v for k, v in p.items() if k != 'request'}
        if 'classification' in row:
            assert row['classification']['request'] == p['request']
        if 'classification' not in row or 'error' in row['classification']:
            if 'classification' in row:
                row.setdefault('classification_failed_attempts', []).append(row['classification'])
            row['classification'] = invoke(p['request'], key)
        if 'error' not in row['classification']:
            req = citation_request(p, row['classification'])
            if req['questions'] and ('citations' not in row or 'error' in row['citations']):
                if 'citations' in row:
                    row.setdefault('citation_failed_attempts', []).append(row['citations'])
                keys = list(req['questions'])
                batches = []
                for offset in range(0, len(keys), 4):
                    batch = {**req, 'questions': {k: req['questions'][k] for k in keys[offset:offset+4]}}
                    batches.append(invoke(batch, key))
                row['citations'] = {
                    'batches': batches,
                    'response': {'answers': {k: a for b in batches if 'error' not in b
                                             for k, a in b['response']['answers'].items()}}}
                if any('error' in b for b in batches):
                    row['citations']['error'] = 'citation_batch_failed'
        return row
    with ThreadPoolExecutor(max_workers=3) as pool:
        for row in pool.map(run, prepared):
            rows.append(row)
            output = json.dumps({'rows': rows, 'batch_seconds': time.perf_counter()-start}, indent=2)
            assert key not in output
            args.output.write_text(output)
            print(json.dumps({'finished': len(rows), 'path_id': row['path_id'],
                              'classification_error': row['classification'].get('error'),
                              'citation_error': row.get('citations', {}).get('error')}), flush=True)


if __name__ == '__main__':
    main()
