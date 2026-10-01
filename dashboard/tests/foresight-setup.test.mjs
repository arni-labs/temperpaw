import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/lib/setup-routing.ts',import.meta.url),'utf8');
const compiled=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const {requiresAgentSetup}=await import('data:text/javascript;base64,'+Buffer.from(compiled).toString('base64'));
const empty={has_anthropic_key:false,has_agents:false,has_personalized_soul:false};

test('authenticated Foresight replay is reachable without unrelated agent/provider setup',()=>{
  assert.equal(requiresAgentSetup('/foresight',empty),false);
});
test('other agent pages still require setup and welcome never redirects to itself',()=>{
  for(const path of ['/','/sessions','/agents']) assert.equal(requiresAgentSetup(path,empty),true);
  assert.equal(requiresAgentSetup('/welcome',empty),false);
  assert.equal(requiresAgentSetup('/sessions',{has_anthropic_key:true,has_agents:true,has_personalized_soul:true}),false);
});

test('cold users can reach authenticated provider settings before creating an agent',()=>{
  assert.equal(requiresAgentSetup('/settings',empty),false);
});

test('cold authenticated users can inspect a linked session without opening other agent routes',()=>{
  for (const path of ['/sessions/ss-01a0aaa4-5d00-7823-8714-61e66535818c','/sessions/ss-01a0aaa4-5d00-7823-8714-61e66535818c/']) {
    assert.equal(requiresAgentSetup(path,empty),false,path);
  }
  for (const path of ['/sessions','/sessions/','/sessions/id/tools','/sessions-other/id','/sessions/id%2Fother','/sessions?id=one']) {
    assert.equal(requiresAgentSetup(path,empty),true,path);
  }
});

test('cold authenticated users can inspect Foresight records without unrelated setup',()=>{
  for (const set of ['Worlds','EventNodes','Endpoints','Claims','Paths','Forecasts','LearningRuns']) {
    for (const suffix of ['','/']) {
      const path=`/entities/${set}/record-123${suffix}`;
      assert.equal(requiresAgentSetup(path,empty),false,path);
    }
  }
});

test('Foresight detail access does not exempt collections, unrelated entities, or nested routes',()=>{
  for (const path of ['/entities/Forecasts','/entities/Forecasts/','/entities/Forecasts/id/tools',
    '/entities/Forecasts/id%2Fother','/entities/Forecast/id','/entities/Secrets/id',
    '/entities/Agents/id','/entities-other/Forecasts/id']) {
    assert.equal(requiresAgentSetup(path,empty),true,path);
  }
});
