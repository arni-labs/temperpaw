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

    def test_packaging_failure_recovery_and_result(self):
        workflow = (ROOT / '.github/workflows/railway-redeploy.yml').read_text()
        package = workflow.split('      - name: Build packaged', 1)[1].split('      - name:', 1)[0]
        self.assertIn('id: package', package)
        self.assertIn('PACKAGE_OUTCOME: ${{ steps.package.outcome }}', workflow)
        rollback = workflow.split('        id: rollback\n', 1)[1].split('        run:', 1)[0]
        condition = rollback.split('        if: ', 1)[1].strip()
        mode_expr = "(inputs.step == 'check_staging' && 'check' || inputs.mode)"
        selection = workflow.split('          if [ "${{ ' + mode_expr + ' }}" = check ]; then', 1)[1].split('          read_json()', 1)[0]
        for mode, package_outcome, e2e, recovery, expected in [
            ('deploy', 'failure', 'skipped', 'success', 'rolled_back'),
            ('deploy', 'failure', 'skipped', 'failure', 'rollback_failed'),
            ('deploy', 'failure', 'skipped', 'skipped', 'failed'),
            ('check', 'failure', 'skipped', 'skipped', 'check-failed'),
            ('check', 'success', 'failure', 'skipped', 'check-failed'),
            ('deploy', 'success', 'success', 'skipped', 'verified'),
            ('deploy', 'skipped', 'skipped', 'skipped', 'verified'),
            ('check', 'skipped', 'skipped', 'skipped', 'checked'),
        ]:
            with self.subTest(mode=mode, package=package_outcome, recovery=recovery):
                # Evaluate the workflow's recovery predicate with concrete step outcomes.
                expr = condition.replace('always()', 'True').replace(mode_expr, repr(mode))
                for step, outcome in [('deploy', 'success'), ('check', 'success'),
                                      ('package', package_outcome), ('e2e', e2e)]:
                    expr = expr.replace('steps.' + step + '.outcome', repr(outcome))
                self.assertEqual(eval(expr.replace('&&', ' and ').replace('||', ' or '),
                                      {'__builtins__': {}}),
                                 mode == 'deploy' and package_outcome == 'failure')
                script = 'if [ "' + mode + '" = check ]; then' + selection + '\nprintf "%s" "$result"'
                env = dict(os.environ, DEPLOY_OUTCOME='success', CHECK_OUTCOME='success',
                           PACKAGE_OUTCOME=package_outcome, E2E_OUTCOME=e2e,
                           ROLLBACK_OUTCOME=recovery)
                result = subprocess.run(['bash', '-c', script], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, expected)
                # These are precisely the two results the Factory maps to ok.
                self.assertEqual(result.stdout in ('verified', 'checked'), package_outcome != 'failure' and e2e != 'failure')

    def test_migration_lookup_failure_stops_before_deletion(self):
        script = (ROOT / 'scripts/railway_release.sh').read_text()
        functions = ''
        for name in ('kernel_rev', 'ignores_unknown_migrations', 'migrations_back_to'):
            functions += name + '()' + script.split(name + '()', 1)[1].split('\n}\n', 1)[0] + '\n}\n'
        mock = r'''
known_migration() { if [ "$1" = old ]; then echo 19; else echo 20; fi; }
gh() {
  if [[ "$2" == *Cargo.lock* ]]; then
    [ "$SCENARIO" != pin_failure ] || { echo 'pin unavailable' >&2; return 1; }
    if [ "$SCENARIO" = pin_missing ]; then
      echo 'no kernel pin'
    else
      printf 'name = "temper-store-postgres"\nsource = "git+https://example.test?rev=abc"\n'
    fi
  else
    [ "$SCENARIO" != source_failure ] || { echo 'source unavailable' >&2; return 1; }
    if [ "$SCENARIO" = supported ]; then echo 'migrator.set_ignore_missing(true);';
    else echo 'migrator.run(pool).await'; fi
  fi
}
db_open() { echo DATABASE_OPENED; }
db_close() { :; }
psql() { echo DATABASE_DELETE >&2; echo 20; }
'''
        for scenario in ('pin_failure', 'pin_missing', 'source_failure', 'supported', 'unsupported'):
            for mode in ('', 'auto'):
                with self.subTest(scenario=scenario, mode=mode), tempfile.TemporaryDirectory() as tmp:
                    Path(tmp, 'additive_migrations.txt').write_text('20 additive\n')
                    env = dict(os.environ, EXPECTED_SHA='new', SCENARIO=scenario,
                               MIGRATION_ROLLBACK=mode, POSTGRES_SERVICE_ID='test')
                    command = 'set -euo pipefail\n' + functions + mock + '\nmigrations_back_to old || exit 7\n'
                    result = subprocess.run(['bash', '-c', command, tmp + '/release.sh'],
                                            env=env, capture_output=True, text=True)
                    failed_lookup = scenario in ('pin_failure', 'pin_missing', 'source_failure')
                    ok = scenario == 'supported' or (scenario == 'unsupported' and mode == 'auto')
                    self.assertEqual(result.returncode, 0 if ok else 7, result.stderr)
                    self.assertEqual('could not be checked' in result.stderr, failed_lookup)
                    self.assertEqual('DATABASE_OPENED' in result.stdout, scenario == 'unsupported' and mode == 'auto')
                    self.assertEqual('DATABASE_DELETE' in result.stderr, scenario == 'unsupported' and mode == 'auto')
                    self.assertEqual('DELETE FROM' in result.stderr, scenario == 'unsupported' and mode == '')
                    self.assertEqual('boots beside' in result.stdout, scenario == 'supported')

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
