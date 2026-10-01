import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const source=readFileSync(new URL('../src/lib/foresight.ts',import.meta.url),'utf8');
const compiled=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText;
const {parseEndpoint,parsePath,parseClaim,parseEvent,requiredEvents,loadFutureBundle}=await import('data:text/javascript;base64,'+Buffer.from(compiled).toString('base64'));

test('future and route references come from their actual saved fields',()=>{
  assert.equal(parseEndpoint({Id:'future',BundleFileId:'file-story'}).bundleFileId,'file-story');
  assert.equal(parseEndpoint({Id:'pending'}).bundleFileId,'');
  assert.equal(parsePath({Id:'route',ClaimId:'claim-1'}).claimId,'claim-1');
});

test('claim failures preserve the recorded error and identity for Activity',()=>{
  const error='invalid claim path ids: EOF while parsing a value at line 1 column 0';
  for (const errorFields of [{error_message:error},{ErrorMessage:error}]) {
    const claim=parseClaim({Id:'claim-failed',Status:'Failed',endpoint_id:'future-1',...errorFields});
    assert.equal(claim.id,'claim-failed');
    assert.equal(claim.status,'Failed');
    assert.equal(claim.endpointId,'future-1');
    assert.equal(claim.error,error);
  }
  assert.equal(parseClaim({Id:'claim-ok',Status:'Settled'}).error,'');
});

test('required events preserve recorded order and unresolved references',()=>{
  const events=[
    parseEvent({Id:'later',statement:'Deployment reaches scale',resolve_by:'2027-04-01',probability:'0.7'}),
    parseEvent({Id:'earlier',statement:'Prototype works',resolve_by:'2027-01-01',probability:'0'}),
  ];
  const result=requiredEvents('["earlier","unloaded","later"]',events);
  assert.equal(result.error,'');
  assert.deepEqual(result.items.map(item=>item.id),['earlier','unloaded','later']);
  assert.equal(result.items[0].event.statement,'Prototype works');
  assert.equal(result.items[0].event.probability,0);
  assert.equal(result.items[0].event.date,'2027-01-01');
  assert.equal(result.items[1].event,null);
  assert.equal(result.items[2].event.statement,'Deployment reaches scale');
});

test('malformed route requirements are visible rather than silently omitted',()=>{
  for(const value of ['not JSON','{"id":"e1"}','["e1",null]','[""]']){
    const result=requiredEvents(value,[]);
    assert.notEqual(result.error,'');
    assert.deepEqual(result.items,[]);
  }
  assert.deepEqual(requiredEvents('[]',[]),{items:[],error:''});
});

test('future bundle loader reads the saved file as text and reports authorization failures',async()=>{
  const markdown='# Future report\n\nA saved scenario.\n<script>untrusted text</script>';
  let requested='';
  assert.deepEqual(await loadFutureBundle('file-story',async path=>{
    requested=path;
    return new Response(markdown,{status:200});
  }),{kind:'ready',text:markdown});
  assert.equal(requested,"/tdata/Files('file-story')/$value");
  const denied=await loadFutureBundle('file-story',async()=>new Response('',{status:403}));
  assert.equal(denied.kind,'unavailable');
  assert.match(denied.error,/403/);
  const failed=await loadFutureBundle('file-story',async()=>{throw new Error('Connection lost');});
  assert.deepEqual(failed,{kind:'unavailable',error:'Connection lost'});
});
