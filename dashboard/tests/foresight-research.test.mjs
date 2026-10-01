import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/lib/foresight.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { researchConfiguration, createForesightWorld, startForesightResearch, startForesightExploration, futureProgress, explorationMessage, parseWorld, parseEndpoint } = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'));

function operations() {
  const calls=[];
  return {calls, api:{
    async createEntity(set) { calls.push({set,action:'Create'}); return {Id:'new-world'}; },
    async postEntityAction(set,id,action,body) { calls.push({set,id,action,body}); return {}; },
  }};
}
const input={name:'Research',domain:'Energy',description:'A question',target:'2027-01-01',mode:'observed',budget:100,asOf:'2025-03-01T00:00'};

test('research requires the chosen provider credential and an explicit model', () => {
  assert.equal(researchConfiguration('openai_codex','configured-model',['openai_api_key']).ready,false);
  assert.equal(researchConfiguration('openai_codex','',['openai_codex_access_token']).ready,false);
  assert.equal(researchConfiguration('','configured-model',['openai_codex_access_token']).ready,false);
  assert.deepEqual(researchConfiguration('openai-codex','configured-model',['openai_codex_access_token']),
    {ready:true,provider:'openai_codex',model:'configured-model'});
  assert.equal(researchConfiguration('local_openai','local-model',['local_openai_api_url']).ready,true);
  assert.equal(researchConfiguration('sakana_fugu','model',['sakana_fugu_api_key']).ready,false);
  assert.equal(researchConfiguration('sakana_fugu','model',['sakana_fugu_api_key','sakana_fugu_api_url']).ready,true);
});

test('cold-user observed creation performs no writes; replay needs no provider', async () => {
  const unconfigured=researchConfiguration(null,null,[]);
  const observed=operations();
  await assert.rejects(createForesightWorld(input,unconfigured,observed.api),/configure/i);
  assert.deepEqual(observed.calls,[]);
  const replay=operations();
  assert.equal(await createForesightWorld({...input,mode:'simulated'},unconfigured,replay.api),'new-world');
  assert.equal(replay.calls.at(-1).action,'OpenReplay');
  assert.equal(replay.calls.at(-1).body.last_ingest_date,'2025-03-01T00:00:00Z');
});

test('observed world persists the actual configured provider/model before research starts', async () => {
  const configured=researchConfiguration('openrouter','vendor/model',['openrouter_api_key']);
  const created=operations();
  await createForesightWorld(input,configured,created.api);
  const configure=created.calls.find(call=>call.action==='Configure');
  assert.equal(configure.body.agent_provider,'openrouter');
  assert.equal(configure.body.agent_model,'vendor/model');
  assert.equal(created.calls.some(call=>call.action==='Seed'),false);
  const started=operations();
  await startForesightResearch('created-before-fix',configured,started.api);
  assert.deepEqual(started.calls.map(call=>call.action),['Configure','Seed']);
  assert.equal(started.calls[0].body.agent_model,'vendor/model');
});

test('research never seeds when configuration is unavailable or saving it fails', async () => {
  const blocked=operations();
  await assert.rejects(startForesightResearch('w',researchConfiguration(null,null,[]),blocked.api),/configure/i);
  assert.deepEqual(blocked.calls,[]);
  let seeded=false;
  await assert.rejects(startForesightResearch('w',researchConfiguration('openai','real-model',['openai_api_key']),{
    ...blocked.api,
    async postEntityAction(_set,_id,action) { if(action==='Configure') throw new Error('denied'); seeded=true; },
  }),/denied/);
  assert.equal(seeded,false);
});


const researchWorld = () => parseWorld({
  Id:'active-world', Status:'Active', agent_provider:'openai_codex', agent_model:'configured-model',
});

test('exploration invokes the existing world action with its actual identifier', async () => {
  const started=operations();
  await startForesightExploration(researchWorld(),[],started.api);
  assert.deepEqual(started.calls,[{set:'Worlds',id:'active-world',action:'SampleEndpoints',body:{}}]);
});

test('existing futures block duplicate sampling, including settled or failed rows', async () => {
  for (const status of ['Sampled','Written','UnderRepair','Scored','Weighted','Discarded','Failed','Unknown']) {
    const blocked=operations();
    await assert.rejects(startForesightExploration(researchWorld(),[parseEndpoint({Id:'future-1',Status:status})],blocked.api),/already/i);
    assert.deepEqual(blocked.calls,[]);
  }
});

test('exploration requires an active world with its own research configuration', async () => {
  for (const patch of [{status:'Seeding'},{status:'Failed'},{agentModel:''},{agentProvider:''}]) {
    const blocked=operations();
    await assert.rejects(startForesightExploration({...researchWorld(),...patch},[],blocked.api));
    assert.deepEqual(blocked.calls,[]);
  }
});

test('future progress separates pending work, completed futures, discards and failures', () => {
  const rows=['Sampled','Written','UnderRepair','Scored','Weighted','Discarded','Failed','Unknown']
    .map((Status,i)=>parseEndpoint({Id:'future-'+i,Status}));
  assert.deepEqual(futureProgress(rows),{pending:5,completed:1,discarded:1,failed:1});
  assert.deepEqual(futureProgress([]),{pending:0,completed:0,discarded:0,failed:0});
});

test('a failed world does not advertise running exploration from its saved phase', () => {
  for (const explorationPhase of ['first_pass','deepening','complete','']) {
    const message=explorationMessage({status:'Failed',explorationPhase},true);
    assert.match(message,/failed/i);
    assert.match(message,/saved.*predictions remain available/i);
    assert.doesNotMatch(message,/is exploring|evaluating/i);
  }
});

test('working exploration retains its phase and prediction availability', () => {
  assert.match(explorationMessage({status:'Active',explorationPhase:'first_pass'},true),/evaluating/);
  assert.equal(explorationMessage({status:'Active',explorationPhase:'first_pass'},false),'');
  for (const status of ['Active','RegisteringForecasts']) {
    const message=explorationMessage({status,explorationPhase:'deepening'},true);
    assert.match(message,/is exploring/);
    assert.match(message,/predictions remain available/);
    assert.doesNotMatch(message,/failed/i);
  }
  assert.match(explorationMessage({status:'Active',explorationPhase:'complete'},true),/complete/);
});
