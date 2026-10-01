import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/lib/foresight.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { parseForecast, forecastGroups, parseLearningRun, probability, sourceLinks, parseDataset, parseWorld, utcTime, readForesightLocation, writeForesightLocation, predictionInputProbability, outcomeFormDefaults } = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'));

test('unknown and malformed probabilities never look like zero certainty', () => {
  for (const value of [null, undefined, '', '  ', 'NaN', 'Infinity', -0.1, 1.01]) assert.equal(probability(value), null);
  assert.equal(probability('0'), 0);
  assert.equal(probability(1), 1);
});
test('world title and model are real fields, not invented summaries', () => {
  const world = parseWorld({ Id:'w', Status:'Active', domain:'Energy storage', model_json:'{"version":"m1","slope":0.8,"intercept":0.1}' });
  assert.equal(world.title, 'Energy storage');
  assert.equal(world.model.version, 'm1');
  assert.equal(parseWorld({Id:'w'}).model, null);
});
test('prediction history keeps revisions and separates equal questions from different events', () => {
  const rows = [
    {Id:'old', event_node_id:'event-a', question:'Will it happen?', registered_at:'2025-01-01', probability:'0.8'},
    {Id:'new', event_node_id:'event-a', question:'Will it happen?', previous_forecast_id:'old', registered_at:'2025-02-01', probability:'0.6'},
    {Id:'other', event_node_id:'event-b', question:'Will it happen?', registered_at:'2025-02-01', probability:'0.7'},
  ].map(parseForecast);
  const groups = forecastGroups(rows);
  assert.equal(groups.length, 2);
  assert.deepEqual(groups.find(g => g.eventId === 'event-a').revisions.map(f => f.id), ['new','old']);
  assert.equal(groups.find(g => g.eventId === 'event-a').latest.previousId, 'old');
});
test('learning retains a measured zero score and exposes malformed reports', () => {
  const run = parseLearningRun({Id:'l', Status:'Adopted', report_json:JSON.stringify({schema_version:1,decision:'adopted',reason:'Held-out improvement',training_count:12,validation_count:4,skipped_count:0,incumbent_brier:0.25,candidate_brier:0,learned_changes:[{name:'slope',before:1,after:0.8,meaning:'Reduced confidence'}]})});
  assert.equal(run.report.candidateBrier, 0);
  assert.equal(run.report.trainingCount, 12);
  assert.equal(run.report.changes[0].meaning, 'Reduced confidence');
  assert.match(parseLearningRun({Id:'x', report_json:'broken'}).reportError, /report/i);
  assert.equal(parseLearningRun({Id:'x'}).report, null);
});
test('evidence links accept only http(s), preserving labels as text elsewhere', () => {
  assert.deepEqual(sourceLinks('["javascript:alert(1)","https://example.org/a","file:///tmp/private"]'), ['https://example.org/a']);
});
test('dataset validation rejects non-arrays, oversize and malformed JSON before network writes', () => {
  assert.equal(parseDataset(''), '');
  assert.equal(parseDataset('[{"id":"a"}]'), '[{"id":"a"}]');
  assert.throws(() => parseDataset('{}'), /array/i);
  assert.throws(() => parseDataset('oops'), /JSON/i);
  assert.throws(() => parseDataset(JSON.stringify(Array.from({length:513},()=>({})))), /512/);
});

test('simulation loader supplies dated inputs, never fitted models or evaluation results', () => {
  const values=JSON.parse(readFileSync(new URL('../../os-apps/paw-foresight/fixtures/simulated-calibration.json', import.meta.url), 'utf8'));
  assert.ok(values.length >= 24 && values.length <= 512);
  assert.equal(parseDataset(JSON.stringify(values)), JSON.stringify(values));
  for (const value of values) {
    assert.equal(value.evidence_kind, 'simulated');
    assert.ok(value.registered_at < value.resolved_at);
    assert.ok(value.resolved_at <= '2025-03-01T00:00:00Z');
    assert.notEqual(probability(value.base_probability), null);
    assert.ok(value.outcome === 0 || value.outcome === 1);
    assert.ok(value.source_refs.every(ref => ref.startsWith('fixture:')));
    for (const key of ['model_json','report_json','candidate_brier','slope','intercept']) assert.equal(key in value,false);
  }
});

test('action timestamps are fixed UTC seconds accepted by the backend', () => {
  assert.equal(utcTime('2025-03-01T12:34'), '2025-03-01T12:34:00Z');
  assert.equal(utcTime('2025-03-01T12:34:56.789Z'), '2025-03-01T12:34:56Z');
  assert.equal(utcTime('2025-03-01'), '2025-03-01T00:00:00Z');
  assert.throws(() => utcTime('not a date'), /valid UTC/);
});

test('outcome provenance stays separate from prediction provenance', () => {
  const forecast = parseForecast({Id:'p', evidence_kind:'observed', outcome_evidence_kind:'proxy-price', outcome:'yes'});
  assert.equal(forecast.evidence, 'observed');
  assert.equal(forecast.outcomeEvidence, 'proxy-price');
  assert.equal(parseForecast({Id:'legacy'}).outcomeEvidence, '');
});

test('refresh restores the selected world, replay clock and view without losing other URL state', () => {
  const state = {worldId:'world older', asOf:'2025-03-02T00:00', tab:'learning'};
  const href = writeForesightLocation('https://example.test/dashboard/foresight?existing=yes#evidence', state);
  assert.deepEqual(readForesightLocation(href), state);
  assert.equal(new URL(href).searchParams.get('existing'), 'yes');
  assert.equal(new URL(href).hash, '#evidence');
  assert.deepEqual(readForesightLocation(writeForesightLocation(href, {...state, worldId:'next'})), {...state,worldId:'next'});
});
test('invalid URL view and replay dates cannot become executable UI state', () => {
  assert.deepEqual(readForesightLocation('https://example.test/dashboard/foresight?view=unknown&at=bad'), {worldId:'',asOf:'',tab:'world'});
  assert.equal(readForesightLocation('https://example.test/?at=2025-02-31T00:00').asOf, '');
});

test('new forecast questions reject certainty values that registration skips', () => {
  for (const value of [0,100,-1,101,NaN,Infinity]) assert.throws(() => predictionInputProbability(value), /greater than 0.*less than 100/);
  assert.equal(predictionInputProbability(55), 0.55);
  assert.equal(predictionInputProbability(0.1), 0.001);
  assert.ok(Math.abs(predictionInputProbability(99.9) - 0.999) < Number.EPSILON);
});

test('outcome forms use the selected replay clock and reset previous answers', () => {
  const now = new Date('2026-09-16T18:00:45Z');
  for (const mode of ['simulated','historical']) {
    const first = outcomeFormDefaults(mode,'2025-06-02T00:00',now);
    assert.deepEqual(first,{time:'2025-06-02T00:00:00',outcome:'yes',sources:''});
    first.time='2026-01-01T00:00'; first.outcome='no'; first.sources='unrelated-evidence';
    assert.deepEqual(outcomeFormDefaults(mode,'2025-07-03T12:30',now),
      {time:'2025-07-03T12:30:00',outcome:'yes',sources:''});
  }
});

test('observed outcome forms use fresh UTC rather than the replay clock or a prior opening', () => {
  assert.equal(outcomeFormDefaults('observed','2025-06-02T00:00',new Date('2026-09-16T18:00:45Z')).time,'2026-09-16T18:00:45');
  assert.equal(outcomeFormDefaults('observed','invalid',new Date('2026-09-16T19:10:20Z')).time,'2026-09-16T19:10:20');
});

test('a replay outcome cannot silently replace a missing replay clock with actual time', () => {
  assert.throws(()=>outcomeFormDefaults('simulated','',new Date('2026-09-16T18:00:00Z')),/valid UTC/);
});
