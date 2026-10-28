import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { resourceProbeInput, resourceBuilderInput, captureAuditResourceProbe } from './audit-release.mjs';
const scope = { window: { start_seconds: 408000, end_seconds: 3000000 }, state:null,user:null,catalog:null,database:null,resource_group:null,native_digest:null,statement_filter:'exclude_doctor_and_non_query' };
const record = { route:'overview',status:200,window_seconds:2592000,sql_leak:false,started_epoch_ms:3000000123,ended_epoch_ms:3000001900,
  scope,resource_request:{scope,primary:{state:'occurrences',sort:'query_time_desc',limit:20},family_top_sort:'execution_count_desc'},audit_timestamp_time_zone:{session_time_zone:'SYSTEM',system_time_zone:'UTC',current_utc_offset_seconds:0} };
const built = ['coverage','primary','concurrency','family_top'].map((role,i)=>`${role}\t${Buffer.from('fixture'+i).toString('hex')}`).join('\n');
test('resource input derives exact rounded execution window and four hashes, no DTO metrics',()=>{
  const x=resourceProbeInput({...record,metrics:{poison:'ignored'}},built);
  assert.deepEqual(x.execution_window,{start_seconds:3000000,end_seconds:3000002});
  assert.equal(x.attribution,'resource_observation_only');assert.equal(x.primary_limit,20);
  assert.equal(Object.keys(x.statement_hashes).length,4);assert.equal(JSON.stringify(x).includes('poison'),false);
  assert.equal(resourceBuilderInput(record).split('\n').length,13);
  assert.equal(resourceBuilderInput(record).split('\n')[11],'exclude_doctor_and_non_query');
  assert.throws(()=>resourceBuilderInput({...record,resource_request:{...record.resource_request,scope:{...scope,statement_filter:undefined}}}));
  for(const patch of [{window_seconds:3600},{sql_leak:true},{ended_epoch_ms:3000400000},{resource_request:{...record.resource_request,primary:{state:'families'}}}])assert.throws(()=>resourceProbeInput({...record,...patch},built));
  assert.throws(()=>resourceProbeInput(record,built.replace('primary','coverage')));
});
test('default skips resource processes and preserves receipt',async()=>{const receipt={};await captureAuditResourceProbe({receipt,env:{},execute:()=>{throw Error('must not run')}});assert.deepEqual(receipt,{});});
test('owned probe launches builder once and read-only probe once, links evidence before continuing',async()=>{
  const root=mkdtempSync(join(tmpdir(),'resource-pure-'));
  try{
    const identity=join(root,'identity.json');writeFileSync(identity,JSON.stringify({runtime:{api_owner:'this_script',verification:'verified',process_id:1}}));
    const builder=join(root,'builder.exe');const receipt={project:'connected-readonly-docker'};let calls=0;
    await captureAuditResourceProbe({receipt,runRoot:root,evaluate:async()=>record,env:{E2E_AUDIT_RESOURCE_BUILDER:builder,E2E_AUDIT_RESOURCE_OWNED:'1',E2E_BUILD_IDENTITY_PATH:identity},execute:(exe,args,options)=>{
      calls++;assert.equal(options.windowsHide,true);
      if(exe===builder){assert.equal(options.input,resourceBuilderInput(record));return {status:0,stdout:built};}
      assert.equal(args.includes('-BuilderExecutable'),true);
      const inputPath=args[args.indexOf('-InputPath')+1];const outputPath=args[args.indexOf('-OutputPath')+1];
      writeFileSync(outputPath,JSON.stringify({schema:'audit-resource-observation/v1',status:'OBSERVED',attribution:'resource_observation_only',request_attribution:'NOT_VERIFIED',input_sha256:createHash('sha256').update(readFileSync(inputPath)).digest('hex'),rows:[{},{},{},{}]}));return {status:0};
    }});
    assert.equal(calls,2);assert.equal(receipt.audit_release.resource_probe.status,'OBSERVED');assert.match(receipt.audit_release.resource_probe.sha256,/^[a-f0-9]{64}$/);
  }finally{rmSync(root,{recursive:true,force:true});}
});
test('missing owner or builder failure retains inconclusive resource evidence and fails closed',async()=>{
  const receipt={project:'connected-readonly-docker'};
  await assert.rejects(captureAuditResourceProbe({receipt,env:{E2E_AUDIT_RESOURCE_BUILDER:'bad'}}),/audit_resource_probe_failed/);
  assert.equal(receipt.audit_release.resource_probe.failure_stage,'owned_identity');assert.equal(receipt.audit_release.resource_probe.status,'INCONCLUSIVE');assert.equal(receipt.audit_release.resource_probe.request_attribution,'NOT_VERIFIED');
});
test('resource hook precedes A05 and runner gates opt-in plus receipt hash',()=>{
  const browser=readFileSync(new URL('./browser-e2e.mjs',import.meta.url),'utf8');
  assert.ok(browser.indexOf('await captureAuditResourceProbe({')<browser.indexOf('await applyAuditReleaseCohort("workload_user")'));
  const module=readFileSync(new URL('./audit-release.mjs',import.meta.url),'utf8');assert.ok(module.includes('...receipt.audit_release, schema:'));
  const runner=readFileSync(new URL('../../scripts/e2e/run-browser-e2e.ps1',import.meta.url),'utf8');
  assert.ok(runner.includes('[string]$AuditResourceBuilder'));assert.ok(runner.includes('requires owned AuditReleaseAcceptance'));
  assert.ok(runner.includes('Resource receipt hash mismatch'));assert.ok(runner.includes('$previousResourceEnv[$name]'));
});

test('resource diagnostics retain only fixed stage numeric exit and timeout flag',async()=>{
  const root=mkdtempSync(join(tmpdir(),'resource-diagnostic-'));
  try {
    const identity=join(root,'identity.json');writeFileSync(identity,JSON.stringify({runtime:{api_owner:'this_script',verification:'verified',process_id:1}}));
    for(const result of [{status:7,stderr:'SENSITIVE SQL'},{status:null,error:{code:'ETIMEDOUT',message:'SENSITIVE SQL'}}]) {
      const receipt={project:'connected-readonly-docker'};
      await assert.rejects(captureAuditResourceProbe({receipt,runRoot:root,evaluate:async()=>record,env:{E2E_AUDIT_RESOURCE_BUILDER:join(root,'builder.exe'),E2E_AUDIT_RESOURCE_OWNED:'1',E2E_BUILD_IDENTITY_PATH:identity},execute:()=>result}),/audit_resource_probe_failed/);
      const diagnostic=receipt.audit_release.resource_probe;
      assert.equal(diagnostic.failure_stage,'builder_export');assert.equal(diagnostic.process_exit,result.status);
      assert.equal(diagnostic.process_timeout,result.status===null);assert.equal(JSON.stringify(diagnostic).includes('SENSITIVE'),false);
    }
    const source=readFileSync(new URL('./audit-release.mjs',import.meta.url),'utf8');
    for(const stage of ['owned_identity','overview_observation','builder_export','input_write','probe_process','probe_receipt'])assert.ok(source.includes(`resourceStage = '${stage}'`));
  } finally {rmSync(root,{recursive:true,force:true});}
});
test('observed one-hour Overview preserves 30d NOT_RUN boundary without additional processes or requests',async()=>{
  const root=mkdtempSync(join(tmpdir(),'resource-window-'));
  try {
    const identity=join(root,'identity.json');writeFileSync(identity,JSON.stringify({runtime:{api_owner:'this_script',verification:'verified',process_id:1}}));
    const receipt={project:'connected-readonly-docker'};
    await captureAuditResourceProbe({receipt,runRoot:root,evaluate:async()=>({...record,window_seconds:3600}),env:{E2E_AUDIT_RESOURCE_BUILDER:join(root,'builder.exe'),E2E_AUDIT_RESOURCE_OWNED:'1',E2E_BUILD_IDENTITY_PATH:identity},execute:()=>{throw Error('must not launch');}});
    assert.equal(receipt.audit_release.resource_probe.status,'NOT_RUN');assert.equal(receipt.audit_release.resource_probe.reason,'observed_overview_not_thirty_days');assert.equal(receipt.audit_release.resource_probe.observed_window_seconds,3600);
  } finally {rmSync(root,{recursive:true,force:true});}
});
