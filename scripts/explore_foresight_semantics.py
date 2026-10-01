#!/usr/bin/env python3
"""Offline semantic-program experiment. No production entity mutations.

A single reasoning session proposes a forest. Small Jev functions inspect it,
recurse over prerequisites, reuse exact computations, and return explicit open
questions. Proposed follow-up research is recorded, not silently executed.
"""
import argparse,hashlib,json,time
from pathlib import Path
from localize_foresight_jev import invoke

GAPS={'none':'No specific causal gap identified in this hypothesis given the supplied evidence. This does not mean it will happen.',
      'timing':'A stated interval is materially too short for the mechanism supplied.',
      'prerequisite':'A necessary causal prerequisite is missing, beyond those explicitly listed.',
      'evidence':'The supplied evidence does not establish the key premise needed to assess this mechanism.',
      'uncertain':'The supplied evidence is insufficient to distinguish these answers.'}
class Program:
    def __init__(self, evidence, provider, max_calls=90, max_depth=5, time_budget=120):
        self.evidence=evidence;self.provider=provider;self.max_calls=max_calls;self.max_depth=max_depth;self.time_budget=time_budget
        self.on_event=None;self.started=time.perf_counter();self.events=[];self.calls=[];self.cache={};self.visiting=set();self.results={}
    def emit(self,candidate,node,function,result,depth=0,**extra):
        self.events.append(dict(at=round(time.perf_counter()-self.started,4),candidateId=candidate['id'],nodeId=node,function=function,result=result,depth=depth,**extra))
        if self.on_event:self.on_event()
    def call(self,candidate,node,function,instruction,options,context,depth):
        request={'model':'jev-1.13.0','state':{'evidence':self.evidence,**context},'questions':{'result':{'type':'choice','instructions':instruction,'criteria':options}}}
        digest=hashlib.sha256(json.dumps(request,sort_keys=True).encode()).hexdigest()
        if digest in self.cache:
            result=self.cache[digest];self.emit(candidate,node,function,result['choice'],depth,kind='cache',requestHash=digest);return result
        if len(self.calls)>=self.max_calls or time.perf_counter()-self.started>=self.time_budget:
            self.emit(candidate,node,function,'budget_exhausted',depth,kind='limit');return {'choice':'uncertain'}
        response=self.provider(request);self.calls.append(response)
        if 'error' in response:
            self.emit(candidate,node,function,'provider_error',depth,kind='error',callIndex=len(self.calls)-1);return {'choice':'uncertain'}
        answer=response['response']['answers']['result'];choice=answer['choice']
        # Confidence is an operational uncertainty gate, never a forecast probability.
        if answer['probabilities'][choice]<.65:choice='uncertain'
        result={**answer,'choice':choice};self.cache[digest]=result
        self.emit(candidate,node,function,choice,depth,kind='semantic',model='jev-1.13.0',distribution=answer['probabilities'],selected=answer['choice'],seconds=response['seconds'],callIndex=len(self.calls)-1,requestHash=digest)
        return result
    def inspect(self,candidate,node_id,depth=0):
        key=(candidate['id'],node_id)
        if key in self.visiting:
            self.emit(candidate,node_id,'check_cycle','cycle_detected',depth,kind='limit');return 'unresolved'
        if key in self.results:
            self.emit(candidate,node_id,'reuse_prerequisite',self.results[key],depth,kind='cache');return self.results[key]
        if depth>self.max_depth or len(self.calls)>=self.max_calls or time.perf_counter()-self.started>=self.time_budget:
            self.emit(candidate,node_id,'check_budget','unresolved',depth,kind='limit');return 'unresolved'
        nodes={n['id']:n for n in candidate['nodes']}
        if node_id not in nodes:
            self.emit(candidate,node_id,'check_reference','missing_prerequisite',depth,kind='limit');return 'unresolved'
        node=nodes[node_id];self.visiting.add(key)
        self.emit(candidate,node_id,'enter_prerequisite','inspecting',depth,kind='traversal',statement=node['statement'])
        gap=self.call(candidate,node_id,'classify_gap','Classify the most consequential gap in state.node as a hypothetical future causal step. Use listed prerequisites and dated evidence. Future claims are hypotheses, not observations. Do not call a future step false merely because it has not happened. Distinguish a specific missing prerequisite from ordinary uncertainty. Existing mitigations matter.',GAPS,{'candidate':{k:v for k,v in candidate.items() if k!='nodes'},'node':node,'prerequisites':[nodes[i] for i in node['requires'] if i in nodes]},depth)
        child_results=[self.inspect(candidate,i,depth+1) for i in node['requires']]
        self.visiting.remove(key)
        result='no_gap_identified' if gap['choice']=='none' and all(v=='no_gap_identified' for v in child_results) else 'unresolved'
        self.results[key]=result;self.emit(candidate,node_id,'combine_prerequisites',result,depth,kind='deterministic',localGap=gap['choice'],children=child_results)
        return result
    def run(self,candidates):
        prior=[]
        for c in candidates:
            novelty=self.call(c,c['root'],'compare_mechanism','Compare state.candidate with state.previous_candidates. Does it use a materially different causal mechanism and lead to a distinguishable outcome? Different wording, dates, or thresholds alone do not establish novelty. If previous_candidates is empty, select distinct.',{'distinct':'A materially different mechanism and distinguishable outcome.','same':'Essentially an existing mechanism restated.','uncertain':'Cannot distinguish reliably.'},{'candidate':c,'previous_candidates':prior},0)
            result=self.inspect(c,c['root'])
            next_step=self.call(c,c['root'],'choose_next_operation','Select the highest-value next operation for this hypothetical candidate using the check results. This schedules a follow-up, it does not claim the follow-up has happened.',{'research':'Obtain external evidence for a consequential unsupported premise.','repair':'Reason about a missing causal mechanism or revise timing.','monitor':'Retain as a hypothesis and monitor its explicit early signals.','uncertain':'Escalate because the next operation is unclear.'},{'candidate':c,'checks':[e for e in self.events if e['candidateId']==c['id'] and e['function']=='classify_gap']},0)
            c['assessment']={'status':result,'novelty':novelty['choice'],'nextOperation':next_step['choice'],'validated':False}
            prior.append({'id':c['id'],'claim':c['claim'],'mechanism':c['mechanism']})
        return {'candidates':candidates,'events':self.events,'calls':self.calls,'elapsed_seconds':time.perf_counter()-self.started,'limits':{'max_calls':self.max_calls,'max_depth':self.max_depth,'time_budget':self.time_budget},'provider_calls':len(self.calls),'cache_hits':sum(e.get('kind')=='cache' for e in self.events)}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('seed',type=Path);p.add_argument('output',type=Path);p.add_argument('--key-file',type=Path,required=True);p.add_argument('--watch-recording',type=Path,help='Atomically update an Observatory recording for local live observation');args=p.parse_args()
    seed=json.loads(args.seed.read_text());raw=seed['result']['result'].strip()
    if raw.startswith('```'):raw=raw.split('\n',1)[1].rsplit('```',1)[0]
    candidates=json.loads(raw)['candidates'];assert len(candidates)==8
    key=args.key_file.read_text().strip();program=Program(seed['context'],lambda req:invoke(req,key))
    if args.watch_recording:
        import datetime
        def snapshot(complete=False):
            data={'schema':'foresight-observatory-v1','id':seed['session_id']+'-live','asOf':datetime.datetime.now(datetime.timezone.utc).isoformat(),'world':{'title':'Enterprise autonomy','cutoff':seed['context']['observation_cutoff'],'target':seed['context']['target_date']},'provenance':{'mode':'LIVE OFFLINE EXPERIMENT','complete':complete},'run':{'seedSeconds':seed['elapsed_seconds'],'semanticSeconds':time.perf_counter()-program.started,'providerCalls':len(program.calls),'cacheHits':sum(e.get('kind')=='cache' for e in program.events)},'candidates':candidates,'events':program.events,'calls':program.calls,'evidence':seed['context'],'forecasts':[],'reviews':[]}
            args.watch_recording.parent.mkdir(parents=True,exist_ok=True);tmp=args.watch_recording.with_suffix('.tmp');tmp.write_text(json.dumps(data,ensure_ascii=False));tmp.replace(args.watch_recording)
        program.on_event=snapshot;snapshot()
    out=program.run(candidates)
    if args.watch_recording:snapshot(True)
    out.update(seed_session_id=seed['session_id'],seed_seconds=seed['elapsed_seconds'],evidence=seed['context'])
    args.output.write_text(json.dumps(out,indent=2));print(json.dumps({k:v for k,v in out.items() if k in ['elapsed_seconds','provider_calls','cache_hits']}))
if __name__=='__main__':main()
