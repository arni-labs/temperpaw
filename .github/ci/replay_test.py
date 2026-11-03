"""python3 -m unittest discover -s .github/ci -p 'replay_test.py'"""

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import replay  # noqa: E402

SPEC = """
[automaton]
name = "{{App}}Release"
states = ["Unmade", "Queued", "Planned", "Done"]
initial = "Unmade"

[[state]]
name = "title"
type = "string"
initial = ""

[[action]]
name = "Ship"
kind = "input"
from = ["Unmade"]
to = "Queued"
effect = ["driven = true", "schedule('Check', 1)"]

[[action]]
name = "Finish"
kind = "input"
from = ["Queued"]
to = "Done"
"""


def keys(old, new):
    return [key for key, _ in replay.changes("x.ioa.toml", replay.parse(old), replay.parse(new) if new is not None else None)]


class Changes(unittest.TestCase):
    def test_a_new_action_state_field_guard_or_schedule_is_safe(self):
        new = SPEC.replace('"Done"]', '"Done", "Held"]').replace(
            "effect = [\"driven = true\", \"schedule('Check', 1)\"]",
            "guard = \"title != ''\"\neffect = [\"driven = true\", \"schedule('Check', 5)\"]",
        ) + '\n[[action]]\nname = "Hold"\nkind = "input"\nfrom = ["Queued"]\nto = "Held"\n\n[[state]]\nname = "note"\ntype = "string"\ninitial = ""\n'
        self.assertEqual(keys(SPEC, new), [])

    def test_timer_tables_do_not_rewrite_records(self):
        for timer in ('{ type = "schedule", action = "Check", delay_seconds = 1 }',
                      '{ type = "schedule_at", action = "Check", at = "2026-10-24T12:00:00Z" }'):
            with self.subTest(timer=timer):
                old = SPEC.replace("\"schedule('Check', 1)\"", timer)
                changed = old.replace('delay_seconds = 1', 'delay_seconds = 5').replace('12:00:00Z', '13:00:00Z')
                self.assertEqual(keys(old, changed), [])
                self.assertEqual(keys(old, old.replace(', ' + timer, '')), [])
                self.assertEqual(keys(old, old.replace('"driven = true"', '"driven = false"')), ["x.ioa.toml action Ship"])

    def test_other_table_effects_still_count(self):
        for effect in ('{ type = "assign", field = "title", value = "old" }',
                       '{ field = "title", value = "old" }'):
            with self.subTest(effect=effect):
                old = SPEC.replace('"driven = true"', effect)
                self.assertEqual(keys(old, old.replace('value = "old"', 'value = "new"')), ["x.ioa.toml action Ship"])

    def test_an_existing_actions_to_is_a_rewrite(self):
        # v43: Ship's to changed, and every shipped release read Planned.
        self.assertEqual(keys(SPEC, SPEC.replace('to = "Queued"', 'to = "Planned"')), ["x.ioa.toml action Ship"])

    def test_a_place_taken_out_of_from_or_a_changed_effect_is_a_rewrite(self):
        self.assertEqual(keys(SPEC, SPEC.replace('from = ["Queued"]', 'from = ["Planned"]')), ["x.ioa.toml action Finish"])
        self.assertEqual(keys(SPEC, SPEC.replace('"driven = true"', '"driven = false"')), ["x.ioa.toml action Ship"])

    def test_a_wider_from_or_an_effect_on_a_new_field_is_safe(self):
        self.assertEqual(keys(SPEC, SPEC.replace('from = ["Queued"]', 'from = ["Queued", "Planned"]')), [])
        fresh = SPEC.replace('"driven = true"', '"driven = true", "shipped = true"') + '\n[[state]]\nname = "shipped"\ntype = "bool"\ninitial = false\n'
        self.assertEqual(keys(SPEC, fresh), [])
        # The same statement on a field the records already hold rewrites them.
        self.assertEqual(keys(SPEC, SPEC.replace('"driven = true"', '"driven = true", "title = \'\'"')), ["x.ioa.toml action Ship"])

    def test_a_gone_action_with_effects_state_field_initial_or_type_is_a_rewrite(self):
        # An action with no effects may go: its events keep the to they were stored with.
        self.assertEqual(keys(SPEC, SPEC.split("[[action]]\nname = \"Finish\"")[0]), [])
        head, _ship, finish = SPEC.split("[[action]]")
        self.assertEqual(keys(SPEC, head + "[[action]]" + finish), ["x.ioa.toml action Ship"])
        self.assertEqual(keys(SPEC, SPEC.replace(', "Done"]', ']').replace('to = "Done"', 'to = "Queued"')), ["x.ioa.toml state Done", "x.ioa.toml action Finish"])
        self.assertEqual(keys(SPEC, SPEC.replace('name = "title"', 'name = "name"')), ["x.ioa.toml field title"])
        # A gone field is reported once, not again for each effect that set it.
        titled = SPEC.replace('"driven = true"', '"driven = true", "title = \'x\'"')
        self.assertEqual(keys(titled, SPEC.replace('name = "title"', 'name = "name"')), ["x.ioa.toml field title"])
        self.assertEqual(keys(SPEC, SPEC.replace('initial = "Unmade"', 'initial = "Queued"')), ["x.ioa.toml initial"])
        self.assertEqual(keys(SPEC, None), ["x.ioa.toml spec"])


class Main(unittest.TestCase):
    """The check on a real repository: a base commit and a head commit."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.cwd = os.getcwd()
        os.chdir(self.dir.name)
        run = lambda *a: subprocess.run(["git", *a], check=True, capture_output=True)
        run("init", "-q")
        run("config", "user.email", "t@t")
        run("config", "user.name", "t")
        os.makedirs("specs")
        open("specs/release.ioa.toml", "w").write(SPEC)
        run("add", ".")
        run("commit", "-qm", "base")
        self.base = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
        open("specs/release.ioa.toml", "w").write(SPEC.replace('to = "Queued"', 'to = "Planned"'))
        run("commit", "-qam", "head")
        self.head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()

    def tearDown(self):
        os.chdir(self.cwd)
        self.dir.cleanup()

    def test_a_changed_to_fails_unless_the_description_says_why(self):
        # What the check prints stays here: on GitHub an ::error line would read as an error of the pull request.
        with contextlib.redirect_stdout(io.StringIO()) as said:
            self.assertEqual(replay.main(self.base, self.head, ""), 1)
            self.assertEqual(replay.main(self.base, self.head, "Replay intended: specs/release.ioa.toml action Ship:"), 1)
            for suffix in ("\n## Verification", "\n\nUnrelated prose", " \t\nMore prose"):
                self.assertEqual(replay.main(self.base, self.head, "Replay intended: specs/release.ioa.toml action Ship:" + suffix), 1)
            self.assertEqual(replay.main(self.base, self.head, "Replay intended: specs/release.ioa.toml action Ship: no record was ever shipped"), 0)
        self.assertIn("specs/release.ioa.toml action Ship: from ['Unmade'] to Queued", said.getvalue())


if __name__ == "__main__":
    unittest.main()
