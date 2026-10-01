#!/usr/bin/env python3
"""Package recorded experiment evidence for the read-only Observatory."""
import argparse,datetime,json
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('evidence',type=Path);p.add_argument('output',type=Path);a=p.parse_args();r=a.evidence
program=json.loads((r/'observatory-program.json').read_text());baseline=json.loads((r/'run-c-baseline.json').read_text());benchmark=json.loads((r/'selective-summary.json').read_text());forecasts=[]
for row in baseline['forecasts']:
 f=row['fields'];prob=f.get('probability');sources=json.loads(f.get('outcome_source_refs') or '[]')
 forecasts.append({'id':row['entity_id'],'eventId':f.get('event_node_id'),'question':f['question'],'probability':float(prob) if prob not in [None,''] else None,'registeredAt':f.get('registered_at'),'resolveBy':f.get('resolve_by'),'resolvedAt':f.get('resolved_at'),'outcome':f.get('outcome'),'evidenceKind':f.get('evidence_kind'),'outcomeEvidenceKind':f.get('outcome_evidence_kind'),'outcomeSources':sources,'outcomeVerified':row.get('status',f.get('Status')) in ['Resolved','Scored'],'model':f.get('model_version')})
# Only governed terminal resolution states assert outcome verification; scoring also checks provenance and dates.
data={'schema':'foresight-observatory-v1','id':'arn518-semantic-program-01','asOf':datetime.datetime.now(datetime.timezone.utc).isoformat(),'world':{'id':'arn518-unattended-20260917-c','title':'Enterprise autonomy','cutoff':program['evidence']['observation_cutoff'],'target':program['evidence']['target_date']},'provenance':{'mode':'OFFLINE EXPERIMENT REPLAY','seedSessionId':program['seed_session_id'],'model':'jev-1.13.0','source':'Frozen Foresight evidence; generated candidate hypotheses; live Jev calls. No live production stream.'},'run':{'seedSeconds':program['seed_seconds'],'semanticSeconds':program['elapsed_seconds'],'providerCalls':program['provider_calls'],'cacheHits':program['cache_hits'],'limits':program['limits']},'candidates':program['candidates'],'events':program['events'],'calls':program['calls'],'evidence':program['evidence'],'forecasts':forecasts,'benchmark':{k:v for k,v in benchmark.items() if k not in ['pairs','missed_baseline_findings','additional_findings','validation_failures']},'reviews':[]}
a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(data,ensure_ascii=False,separators=(',',':')))

# Retain original endpoint excerpts for explicitly unmatched blind review.
import re
data=json.loads(a.output.read_text());packets=json.loads((r/'fair-packets.json').read_text());ends=json.loads((r/'fair-endpoints.json').read_text());baseline=[]
for i,e in enumerate(ends):
 f=e['fields'];endpoint=f['Id'];matching=[p for p in packets if p['packet']['state'].get('endpoint_id')==endpoint or endpoint in p['packet']['state']['endpoint_bundle']]
 if not matching:continue
 state=matching[0]['packet']['state'];claims=json.loads(f.get('claims_json','[]'));claim=claims[0]['text'];bundle=state['endpoint_bundle'];body=bundle.split('## Document 1',1)[-1];paras=[p.strip() for p in body.split('\n\n') if len(p.strip())>180 and not p.strip().startswith(('#','**Date','World:'))];scene=paras[0] if paras else 'No narrative excerpt found.'
 required=state['required_nodes'];ids={n['id'] for n in required};nodes=[{'id':n['id'],'statement':n['statement'],'requires':[k for k in json.loads(n['edges']) if k in ids],'evidence_note':'Original recorded path prerequisite; hypothetical future claim.'} for n in required]
 root=nodes[-1]['id'];stance=json.loads(f.get('driver_config') or '{}').get('stance','Original endpoint configuration not recorded.')
 baseline.append({'id':endpoint,'title':'Original future '+str(i+1),'claim':claim,'mechanism':stance,'assumption':stance,'scene':'Hypothetical document excerpt: '+scene,'falsifier':'No separate falsifier was recorded for this endpoint. Evaluate against the dated claim above.','signals':[n['statement'] for n in required[:2]],'nodes':nodes,'root':root})
b={'schema':data['schema'],'id':'arn518-original-corridor-endpoints','asOf':data['asOf'],'world':data['world'],'provenance':{'mode':'ORIGINAL CORRIDOR EXCERPTS'},'candidates':baseline,'events':[],'forecasts':data['forecasts'],'reviews':[]}
data['baseline']=b;data['comparisonLimitations']='Original corridor endpoint excerpts versus new short hypothesis proposals. Same world and evidence cutoff, but unequal generation budgets and different output formats. These preferences are exploratory, not evidence of system superiority.';a.output.write_text(json.dumps(data,ensure_ascii=False,separators=(',',':')));print('baseline endpoints',len(baseline))
