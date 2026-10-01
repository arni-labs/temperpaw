import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { once } from 'node:events';
import vm from 'node:vm';
import ts from 'typescript';

// Execute the real shared client against HTTP, varying the server's configured tenant.
// Only unrelated view/format imports are replaced; transport and entity parsing are real.
test('authenticated requests defer tenant selection to the server for default and dedicated deployments', async () => {
  let configuredTenant = 'default';
  let actionStatus = 200;
  const received = [];
  const server = createServer((req, res) => {
    received.push({tenant:req.headers['x-tenant-id'], principal:req.headers['x-temper-principal-kind'], method:req.method});
    res.setHeader('content-type','application/json');
    if (req.method === 'POST') res.statusCode = actionStatus;
    res.end(JSON.stringify({tenant:configuredTenant}));
  });
  server.listen(0,'127.0.0.1');
  await once(server,'listening');
  try {
    const address = server.address();
    assert.equal(typeof address, 'object');
    const origin = 'http://127.0.0.1:' + address.port;
    const module = {exports:{}};
    const source = readFileSync(new URL('../src/lib/api.ts', import.meta.url), 'utf8');
    const compiled = ts.transpileModule(source, {compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
    const requestOptions = [];
    vm.runInNewContext(compiled, {
      exports:module.exports, module,
      require:(name) => name.includes('paw-patrol') ? {pawPatrolView:{name:'paw-patrol'}} : {entitySetToEntityType:(name)=>name},
      fetch:(url, options) => { requestOptions.push(options); return fetch(new URL(url,origin),options); },
      URLSearchParams, Headers, console,
    });
    for (const tenant of ['default','deep-sci-fi']) {
      configuredTenant = tenant;
      const response = await module.exports.apiFetch('/tdata/Worlds');
      assert.equal((await response.json()).tenant, tenant);
    }
    assert.equal(received.length,2);
    for (const request of received) {
      assert.equal(request.tenant,undefined);
      assert.equal(request.principal,'human');
      assert.equal(request.method,'GET');
    }
    assert.ok(requestOptions.every(options => options.credentials === 'same-origin'));
    actionStatus = 403;
    await assert.rejects(module.exports.postEntityAction('Worlds','world-1','ResumeSeed',{}),
      error => error.status === 403 && error.message.includes('OData action failed'));

  } finally { server.close(); await once(server,'close'); }
});
