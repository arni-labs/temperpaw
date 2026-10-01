import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/lib/foresight.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, {compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const {parseWorld, loadResearchSession, researchSessionProblem, canRetryResearch, retryForesightResearch} = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'));

test('world research uses its recorded session ID, not names or prompt text', async () => {
  const world = parseWorld({Id:'world-1',Status:'Seeding',ResearchSessionId:'session-1'});
  const calls=[];
  const state = await loadResearchSession(world.researchSessionId, async (set,id) => {
    calls.push({set,id});
    return {Id:id,Status:'Failed',error_message:'Provider rejected the request',turn_count:'3'};
  });
  assert.deepEqual(calls,[{set:'Sessions',id:'session-1'}]);
  assert.equal(state.kind,'ready');
  assert.equal(state.id,'session-1');
  assert.equal(state.status,'Failed');
  assert.equal(state.turns,3);
  assert.match(researchSessionProblem(state,'Seeding'),/Provider rejected the request/);
});

test('unlinked worlds remain unknown without searching for matching text', async () => {
  let queried=false;
  const world=parseWorld({Id:'world-1',Status:'Seeding',name:'Surveyor-session-1'});
  const state=await loadResearchSession(world.researchSessionId,async()=>{queried=true;return {};});
  assert.deepEqual(state,{kind:'unlinked'});
  assert.equal(queried,false);
});

test('a denied, missing or mismatched session cannot look healthy', async () => {
  for (const fetch of [
    async()=>{throw new Error('OData get failed: 403');},
    async()=>({Id:'other-session',Status:'Completed'}),
    async()=>({Id:'session-1'}),
  ]) {
    const state=await loadResearchSession('session-1',fetch);
    assert.equal(state.kind,'unavailable');
    assert.equal(state.id,'session-1');
    assert.ok(researchSessionProblem(state,'Seeding'));
  }
});

test('failed and cancelled sessions stay visible even without error text', async () => {
  for (const status of ['Failed','Cancelled']) {
    const state=await loadResearchSession('session-1',async()=>({Id:'session-1',Status:status}));
    assert.match(researchSessionProblem(state,'Seeding'),new RegExp(status,'i'));
  }
});

test('a completed session does not imply the world finished seeding', async () => {
  const state=await loadResearchSession('session-1',async()=>({Id:'session-1',Status:'Completed'}));
  assert.match(researchSessionProblem(state,'Seeding'),/last (reported|recorded) research session completed/i);
  assert.equal(researchSessionProblem(state,'Active'),'');
});

test('running research retains actual progress without an invented failure', async () => {
  const state=await loadResearchSession('session-1',async()=>({Id:'session-1',Status:'Executing',turn_count:2,last_heartbeat_at:'2026-09-16T12:00:00Z'}));
  assert.equal(state.status,'Executing');
  assert.equal(state.heartbeat,'2026-09-16T12:00:00Z');
  assert.equal(researchSessionProblem(state,'Seeding'),'');
});


const failedResearchWorld = () => parseWorld({Id:'world-1',Status:'Seeding',research_session_id:'failed-1'});
const failedResearchSession = () => ({kind:'ready',id:'failed-1',status:'Failed',error:'Provider unavailable',turns:0,heartbeat:''});

test('research retry requires the selected world and its confirmed failed or cancelled session', () => {
  const world=failedResearchWorld(), session=failedResearchSession();
  assert.equal(canRetryResearch(world,session,''),true);
  assert.equal(canRetryResearch(world,{...session,status:'Cancelled'},''),true);
  for (const status of ['Created','Active','Failed','Archived']) {
    assert.equal(canRetryResearch({...world,status},session,''),false);
  }
  for (const status of ['Running','Executing','Completed','Unknown']) {
    assert.equal(canRetryResearch(world,{...session,status},''),false);
  }
  for (const state of [{kind:'unlinked'},{kind:'loading',id:'failed-1'},
    {kind:'unavailable',id:'failed-1',error:'read denied'}, {...session,id:'another-world-session'}]) {
    assert.equal(canRetryResearch(world,state,''),false);
  }
  assert.equal(canRetryResearch(null,session,''),false);
  assert.equal(canRetryResearch(world,session,'failed-1'),false);
});

test('research retry dispatches once and does not reuse the old session after refresh', async () => {
  const requests={}, calls=[], world=failedResearchWorld(), session=failedResearchSession();
  let finish;
  const api={postEntityAction:async (...args)=>{calls.push(args);await new Promise(resolve=>finish=resolve);}};
  const first=retryForesightResearch(world,session,requests,api);
  await assert.rejects(retryForesightResearch(world,session,requests,api),/retry/i);
  assert.deepEqual(calls,[['Worlds','world-1','ResumeSeed',{}]]);
  finish(); await first;
  // A refresh may still return the same failed Session while a replacement starts.
  await assert.rejects(retryForesightResearch({...world},{...session},requests,api),/retry/i);
  assert.equal(calls.length,1);
  assert.equal(canRetryResearch({...world,researchSessionId:'new-2'},session,requests[world.id]),false);
  assert.equal(canRetryResearch({...world,researchSessionId:'new-2'},
    {...session,id:'new-2',status:'Executing'},requests[world.id]),false);
  assert.equal(canRetryResearch({...world,researchSessionId:'new-2'},
    {...session,id:'new-2'},requests[world.id]),true);
});

test('uncertain retry failures remain visible without allowing duplicate dispatch', async () => {
  const requests={}, world=failedResearchWorld(), session=failedResearchSession();
  await assert.rejects(retryForesightResearch(world,session,requests,
    {postEntityAction:async()=>{throw new Error('Connection lost');}}),/Connection lost/);
  assert.equal(canRetryResearch(world,session,requests[world.id]),false);
});


test('a definite HTTP rejection releases the retry guard', async () => {
  for (const status of [400,401,403,409,429]) {
    const requests={}, world=failedResearchWorld(), session=failedResearchSession();
    await assert.rejects(retryForesightResearch(world,session,requests,
      {postEntityAction:async()=>{throw Object.assign(new Error('Request rejected'),{status});}}),/Request rejected/);
    assert.equal(canRetryResearch(world,session,requests[world.id]),true);
  }
});
