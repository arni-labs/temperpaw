import unittest
from route_foresight_jev import route_checks


class RoutingTests(unittest.TestCase):
    def run_policy(self, answer=None, error=False):
        classification = {'response': {'answers': {'t01_lag': answer or {}}}}
        if error:
            classification['error'] = 'invalid_provider_response'
        return route_checks(['t01'], ['lag'], classification)

    def test_threshold_boundary(self):
        for score, skipped in [(0.79, False), (0.80, True), (0.95, True)]:
            result = self.run_policy({'choice': 'clear', 'probabilities':
                                     {'clear': score, 'defect': 1-score, 'unknown': 0}})
            self.assertEqual(bool(result['skipped']), skipped)
            self.assertEqual(bool(result['route']), not skipped)

    def test_failed_response_cannot_skip(self):
        result = self.run_policy({'choice': 'clear', 'probabilities':
                                 {'clear': 1, 'defect': 0, 'unknown': 0}}, error=True)
        self.assertEqual(result['route'], {'t01': ['lag']})

    def test_missing_invalid_and_nonclear_escalate(self):
        for answer in [{}, {'choice': 'unknown'}, {'choice': 'defect'},
                       {'choice': 'clear', 'probabilities': {'clear': 1.2}},
                       {'choice': 'clear', 'probabilities': {'clear': .9, 'defect': .8, 'unknown': 0}}]:
            self.assertEqual(self.run_policy(answer)['route'], {'t01': ['lag']})

    def test_exhaustive_disjoint_assignment(self):
        result = route_checks(['t01', 't02'], ['lag', 'miracle'], {'response': {'answers': {}}})
        self.assertEqual(result, {'route': {'t01': ['lag', 'miracle'], 't02': ['lag', 'miracle']}, 'skipped': {}})


if __name__ == '__main__':
    unittest.main()
