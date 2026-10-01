import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {parse} from 'svelte/compiler';
import ts from 'typescript';

const source=readFileSync(new URL('../src/routes/foresight/+page.svelte',import.meta.url),'utf8');
const page=parse(source,{modern:true});
const helper=page.instance.content.body.find(node=>node.type==='FunctionDeclaration'&&node.id.name==='recordHref');
assert.ok(helper,'Foresight record links must have a testable route builder');
const compiled=ts.transpileModule(source.slice(helper.start,helper.end),{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
const recordHref=new Function('base',`${compiled}; return recordHref;`)('/dashboard');

test('Foresight record links use the collections consumed by entity detail',()=>{
  for (const [type,set] of [
    ['World','Worlds'],['EventNode','EventNodes'],['Endpoint','Endpoints'],
    ['Claim','Claims'],['Path','Paths'],['Forecast','Forecasts'],['LearningRun','LearningRuns'],
  ]) {
    assert.equal(recordHref(type,'record-1'),`/dashboard/entities/${set}/record-1`);
  }
});

test('record links preserve an opaque entity ID within one URL segment',()=>{
  assert.equal(recordHref('Forecast','question/a?b#c'),'/dashboard/entities/Forecasts/question%2Fa%3Fb%23c');
});
