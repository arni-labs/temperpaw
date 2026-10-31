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
        # A forward step whose commit is not the image's own names it as PINNED_COMMIT: the step after checkout deploys
        # it only when the image's temper revs carry it (test_pinned_commit_must_be_carried), before anything deploys.
        carried = workflow.index('run: bash scripts/temper_pin.sh carried')
        self.assertIn("if: env.PINNED_COMMIT != ''", workflow[:carried].rsplit('- name:', 1)[1])
        self.assertLess(carried, workflow.index('run: bash scripts/railway_release.sh deploy'))
        for step, commit, resolved, expected, ok, pinned in [
            ('stage', SHA, SHA, '', True, ''),
            ('check_staging', SHA, SHA, '', True, ''),
            ('go_live', SHA, SHA, '', True, ''),
            ('stage', SHA, 'b' * 40, '', True, SHA),
            ('check_staging', SHA, 'b' * 40, '', True, SHA),
            ('go_live', SHA, 'b' * 40, '', True, SHA),
            ('stage', '', SHA, '', False, ''),
            ('stage', 'a' * 7, SHA, '', False, ''),
            ('roll_back', 'b' * 40, SHA, '', True, ''),
            ('', '', SHA, '', True, ''),
            ('', '', SHA, 'b' * 40, False, ''),
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
                    exported = output.read_text()
                    self.assertIn('EXPECTED_SHA=' + resolved, exported)
                    if pinned:
                        self.assertIn('PINNED_COMMIT=' + pinned, exported)
                    else:
                        self.assertNotIn('PINNED_COMMIT', exported)
                else:
                    self.assertFalse(output.exists(), 'rejected release must not export a deployment SHA')

    def test_pinned_commit_must_be_carried(self):
        # A release commit that is not the image's own (a kernel change, staged through its temper pin) deploys only
        # when every temper rev at the image's commit is that commit or one after it.
        pinned, older, other = 'c' * 40, 'd' * 40, 'e' * 40
        with tempfile.TemporaryDirectory() as tmp:
            origin, work, tools = Path(tmp, 'origin'), Path(tmp, 'work'), Path(tmp, 'bin')
            for d in (origin, work, tools):
                d.mkdir()

            def git(cwd, *args):
                return subprocess.run(['git', '-C', str(cwd), '-c', 'user.name=test', '-c', 'user.email=test', *args],
                                      check=True, capture_output=True, text=True).stdout.strip()

            git(origin, 'init', '-q')
            git(origin, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
            images = {}
            for name, dependency in [('pins', f'temper = {{ git = "https://github.com/arni-labs/temper.git", rev = "{pinned}" }}'),
                                     ('none', 'serde = "1"')]:
                Path(origin, 'Cargo.toml').write_text('[dependencies]\n' + dependency + '\n')
                git(origin, 'add', 'Cargo.toml')
                git(origin, 'commit', '-q', '-m', name)
                images[name] = git(origin, 'rev-parse', 'HEAD')
            git(work, 'init', '-q')
            git(work, 'remote', 'add', 'origin', origin.as_uri())
            # arni-labs/temper's compare as GitHub answers it: a rev that is the commit is identical, one after it ahead.
            gh = Path(tools, 'gh')
            gh.write_text('#!/bin/sh\ncase "$2" in\n'
                          f'  */compare/{pinned}...{pinned}) echo identical ;;\n'
                          f'  */compare/{older}...{pinned}) echo ahead ;;\n'
                          '  *) echo diverged ;;\nesac\n')
            gh.chmod(0o755)
            for image, commit, ok in [
                (images['pins'], pinned, True),
                (images['pins'], older, True),
                (images['pins'], other, False),
                (images['none'], pinned, False),
            ]:
                with self.subTest(image=image, commit=commit):
                    env = dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}', GITHUB_REPOSITORY='test/repo',
                               COMMIT=commit, IMAGE_COMMIT=image, READ_TOKEN='test')
                    result = subprocess.run(['bash', str(ROOT / 'scripts/temper_pin.sh'), 'carried'], cwd=work, env=env,
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)

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

    def test_guest_baseline_needs_a_good_read(self):
        script = (ROOT / 'scripts/railway_release.sh').read_text()
        functions = ''
        for name in ('guest', 'guest_baseline'):
            functions += name + '()' + script.split('\n' + name + '()', 1)[1].split('\n}\n', 1)[0] + '\n}\n'
        # curl answers with the scenario's body, then the status line -w adds.
        mock = r'''
curl() {
  case "$SCENARIO" in
    good) printf '{"ok":true,"data":{"records":[{"thing":"find","records":[1,2]}]}}\n200' ;;
    empty) printf '\n200' ;;
    html) printf '<html>bad gateway</html>\n502' ;;
    refused) printf '\n000'; return 7 ;;
  esac
}
'''
        for scenario, url, ok, read in [
            ('good', 'https://example.test/guest', True, '200 true find 2'),
            ('empty', 'https://example.test/guest', False, '200 false no JSON body'),
            ('html', 'https://example.test/guest', False, '502 false no JSON body'),
            ('refused', 'https://example.test/guest', False, '000 false no JSON body'),
            ('empty', '', True, ''),
        ]:
            with self.subTest(scenario=scenario, url=url):
                command = 'set -euo pipefail\n' + functions + mock + '\nwas="$(guest_baseline)" || exit 7\nprintf "%s" "$was"\n'
                env = dict(os.environ, SCENARIO=scenario, GUEST_CHECK_URL=url)
                result = subprocess.run(['bash', '-c', command], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0 if ok else 7, result.stderr)
                if ok:
                    self.assertEqual(result.stdout, read)
                else:
                    self.assertIn('the guest read answered ' + read + ': no baseline, so no deploy', result.stderr)

    def test_route_table_read_from_probes_when_the_log_line_is_missing(self):
        script = (ROOT / 'scripts/railway_release.sh').read_text()
        functions = ''
        for name in ('routes_serve', 'against_before'):
            functions += name + '()' + script.split('\n' + name + '()', 1)[1].split('\n}\n', 1)[0] + '\n}\n'
        # The table answers a method no route takes with its 404; a built-in path answers 405 with no body.
        mock = r'''
wait_healthy() { :; }
ask_failures() { echo 0; }
route_count() { printf '%s' "$LOGGED"; }
probe() { printf '%s' "$AFTER"; }
curl() {
  case "${*: -1}" in
    *"$TABLE_PATH") [ "$TABLE" = loaded ] && printf '{"error":"no route matches","path":"x"}' ;;
  esac
  return 0
}
'''
        judge = 'POST operator /katagami/judge'
        status = 'GET operator /paw/setup/status'
        for logged, before_count, answers, table, misses in [
            ('11', '11', {judge: '401', status: '200'}, 'loaded', []),
            ('', '11', {judge: '401', status: '200'}, 'loaded', []),
            ('', '', {judge: '401', status: '200'}, 'loaded', []),
            ('10', '11', {judge: '401', status: '200'}, 'loaded', ['route_count 10, before 11']),
            # Only a built-in path answers, or the route path answers 404: nothing shows the table serves.
            ('', '11', {judge: '404', status: '200'}, 'loaded',
             ['the deployment logged no route table for the default tenant, and no signed-in probe reaches a route']),
            ('', '11', {judge: '401', status: '200'}, 'empty',
             ['the deployment logged no route table for the default tenant, and no signed-in probe reaches a route']),
        ]:
            with self.subTest(logged=logged, before=before_count, answers=answers, table=table), tempfile.TemporaryDirectory() as tmp:
                before = {'route_count': before_count, 'probes': {judge: answers[judge], status: '200'}}
                Path(tmp, 'before.json').write_text(__import__('json').dumps(before))
                command = ('set -euo pipefail\n' + functions + mock +
                           '\nmisses=()\nagainst_before dep\nprintf "MISS %s\\n" "${misses[@]+"${misses[@]}"}"\n')
                env = dict(os.environ, STATE_DIR=tmp, LOGGED=logged, TABLE=table, TABLE_PATH='/katagami/judge',
                           AFTER=__import__('json').dumps(answers), PROBE_ROUTES=judge + '\n' + status,
                           BASE_URL='https://example.test', TEMPER_API_KEY='test')
                result = subprocess.run(['bash', '-c', command], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                got = [line[5:] for line in result.stdout.splitlines() if line.startswith('MISS ') and line[5:]]
                self.assertEqual(got, misses, result.stdout)


if __name__ == '__main__':
    unittest.main()
