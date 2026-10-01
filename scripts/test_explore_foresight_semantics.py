import unittest
from explore_foresight_semantics import Program


def provider(request):
    options=request['questions']['result']['criteria'];choice='none' if 'none' in options else next(iter(options))
    return {'request':request,'seconds':.1,'response':{'answers':{'result':{'choice':choice,'probabilities':{k:int(k==choice) for k in options}}}}}

def candidate(nodes):return {'id':'c','root':'root','nodes':nodes,'claim':'Hypothesis','mechanism':'Mechanism'}

class ProgramTests(unittest.TestCase):
    def test_cycle_returns_unresolved(self):
        c=candidate([{'id':'root','statement':'A','requires':['root']}]);p=Program({},provider)
        self.assertEqual(p.inspect(c,'root'),'unresolved');self.assertTrue(any(e['result']=='cycle_detected' for e in p.events))
    def test_shared_prerequisite_reused(self):
        c=candidate([{'id':'root','statement':'A','requires':['a','b']},{'id':'a','statement':'B','requires':['leaf']},{'id':'b','statement':'C','requires':['leaf']},{'id':'leaf','statement':'D','requires':[]}]);p=Program({},provider)
        self.assertEqual(p.inspect(c,'root'),'no_gap_identified');self.assertEqual(len(p.calls),4);self.assertEqual(sum(e['function']=='reuse_prerequisite' for e in p.events),1)
    def test_budget_and_depth_cannot_be_success(self):
        c=candidate([{'id':'root','statement':'A','requires':['leaf']},{'id':'leaf','statement':'B','requires':[]}])
        self.assertEqual(Program({},provider,max_calls=1).inspect(c,'root'),'unresolved')
        self.assertEqual(Program({},provider,max_depth=0).inspect(c,'root'),'unresolved')
    def test_provider_failure_propagates_uncertainty(self):
        c=candidate([{'id':'root','statement':'A','requires':[]}]);p=Program({},lambda q:{'error':'HTTP_503'})
        self.assertEqual(p.inspect(c,'root'),'unresolved')
    def test_exact_request_cache_is_scoped_to_inputs(self):
        p=Program({},provider);c=candidate([{'id':'root','statement':'A','requires':[]}]);args=(c,'root','test','Question',{'none':'No gap'},{'value':'a'},0)
        p.call(*args);p.call(*args);self.assertEqual(len(p.calls),1)
        p.call(c,'root','test','Question',{'none':'No gap'},{'value':'b'},0);self.assertEqual(len(p.calls),2)

if __name__=='__main__':unittest.main()
