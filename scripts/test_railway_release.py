#!/usr/bin/env python3
"""Offline regressions: execute the workflow validator and cleanup in Bash."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(os.environ.get('RAILWAY_TEST_ROOT', Path(__file__).resolve().parents[1]))
SHA = 'a' * 40


class RailwayReleaseTests(unittest.TestCase):
    def test_factory_commit(self):
        workflow = (ROOT / '.github/workflows/railway-redeploy.yml').read_text()
        self.assertIn('FACTORY_STEP: ${{ inputs.step }}', workflow)
        self.assertIn('FACTORY_COMMIT: ${{ inputs.commit }}', workflow)
        # Exercise the real validation step without GitHub interpolation or network access.
        script = textwrap.dedent(workflow.split('        run: |\n', 1)[1].split('\n      - uses:', 1)[0])
        script = re.sub(r'\$\{\{.*?\}\}', 'false', script)
        script = 'gh() { echo "$RESOLVED_SHA"; }\n' + script
        for step, commit, resolved, expected, ok in [
            ('stage', SHA, SHA, '', True),
            ('check_staging', SHA, SHA, '', True),
            ('go_live', SHA, SHA, '', True),
            ('stage', SHA, 'b' * 40, '', False),
            ('check_staging', SHA, 'b' * 40, '', False),
            ('go_live', SHA, 'b' * 40, '', False),
            ('stage', '', SHA, '', False),
            ('stage', 'a' * 7, SHA, '', False),
            ('roll_back', 'b' * 40, SHA, '', True),
            ('', '', SHA, '', True),
            ('', '', SHA, 'b' * 40, False),
        ]:
            with self.subTest(step=step, commit=commit, resolved=resolved, expected=expected), tempfile.TemporaryDirectory() as tmp:
                env = dict(os.environ, IMAGE_TAG='sha-' + resolved[:7],
                           EXPECTED_SHA_INPUT=expected, FACTORY_STEP=step,
                           FACTORY_COMMIT=commit, RESOLVED_SHA=resolved,
                           GITHUB_REPOSITORY='test/repo', GITHUB_ENV=tmp + '/env',
                           RUNNER_TEMP=tmp, ROLLBACK_TAG='', DEPLOY_ENV='staging',
                           BASE_URL='https://example.test', RUN_ARTIFACT_BATCH_E2E='false',
                           POLL_ATTEMPTS='1', POLL_SECONDS='0', SOAK_SECONDS='0')
                for key in ('RAILWAY_API_TOKEN', 'RAILWAY_PROJECT_ID', 'RAILWAY_ENVIRONMENT_ID', 'RAILWAY_SERVICE_ID', 'TEMPER_API_KEY'):
                    env[key] = 'test'
                result = subprocess.run(['bash', '-c', script], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, ok, result.stderr)
                output = Path(tmp + '/env')
                if ok:
                    self.assertIn('EXPECTED_SHA=' + resolved, output.read_text())
                else:
                    self.assertFalse(output.exists(), 'rejected release must not export a deployment SHA')

    def test_proxy_cleanup_in_conditional(self):
        script = (ROOT / 'scripts/railway_release.sh').read_text()
        function = 'db_close()' + script.split('db_close()', 1)[1].split('\n}\n', 1)[0] + '\n}\n'
        for scenario, ok in [('lookup_failure', False), ('delete_failure', False),
                             ('delete_false', False), ('malformed', False),
                             ('absent', True), ('deleted', True)]:
            with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as tmp:
                marker = Path(tmp) / 'proxy-opened'
                marker.touch()
                mock = r'''
gql() {
  if [[ "$1" == *tcpProxyDelete* ]]; then
    [ "$SCENARIO" != delete_failure ] || return 1
    if [ "$SCENARIO" = delete_false ]; then
      echo '{"data":{"tcpProxyDelete":false}}'
    else
      echo '{"data":{"tcpProxyDelete":true}}'
    fi
  else
    [ "$SCENARIO" != lookup_failure ] || return 1
    case "$SCENARIO" in
      malformed) echo '{"data":{}}';;
      absent) echo '{"data":{"tcpProxies":[]}}';;
      *) echo '{"data":{"tcpProxies":[{"id":"test-proxy"}]}}';;
    esac
  fi
}
'''
                # The || context reproduces rollback's suppression of errexit.
                command = 'set -euo pipefail\n' + function + mock + '\ndb_close || exit 7\n'
                env = dict(os.environ, STATE_DIR=tmp, POSTGRES_SERVICE_ID='test',
                           RAILWAY_ENVIRONMENT_ID='test', SCENARIO=scenario)
                result = subprocess.run(['bash', '-c', command], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, ok, result.stderr)
                self.assertEqual(marker.exists(), not ok)
                self.assertEqual('the TCP proxy is removed' in result.stdout, ok)


if __name__ == '__main__':
    unittest.main()
