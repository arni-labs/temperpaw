"""Whether a change to a Temper spec would rewrite the records it already has.

Temper rebuilds a record by replaying its stored events through the spec it runs now. Each event's action applies
its `to` and its assignment effects as they are written today; guards, constraints, Cedar, triggers and schedule()
are not run again. An event whose action is gone, or no longer starts from where the record is, keeps the `to` it
was stored with and loses its effects (temper-server entity_actor/actor.rs, replay_events). So changing an existing
action's from, to or effects, removing an action that had effects, or removing a state, a field or the whole type,
changes records that went through it once they are read again. On 24 Oct 2026 the Factory's v43
changed Ship's `to` and every shipped release read Planned until it was rolled back; katagami #1023's rename and
#1036's Remove plan were caught by hand. New actions, states, fields, guards, constraints, params and Cedar are safe.

    python3 .github/ci/replay.py BASE HEAD [PR_BODY_FILE]

compares every *.ioa.toml that BASE has and HEAD changed or deleted, prints each change that rewrites records, and
exits 1 when one is not marked intended. A change is meant when the pull request's description has a line

    Replay intended: <the change as printed>: <why no record is harmed, or why rewriting them is wanted>

for example `Replay intended: os-apps/paw-fs/specs/file.ioa.toml action Remove: no record can have reached
Removing`. A line with no reason does not count.

The same check runs in arni-labs/katagami (.github/ci/replay.py): change both together.
"""

import json
import re
import subprocess
import sys
import tomllib

SPEC = re.compile(r"\.ioa\.toml$")
MARK = re.compile(r"^[ \t]*Replay intended:[ \t]*(.+?):[ \t]*(\S.*)$", re.MULTILINE)


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def parse(text):
    # Placeholders such as {{App}} stand for names filled in at install: any stable stand-in compares the same.
    return tomllib.loads(re.sub(r"\{\{([^}]*)\}\}", lambda m: "P_" + re.sub(r"\W", "_", m.group(1)), text))


TARGET = re.compile(r"^\s*(?:append\(\s*|remove\(\s*)?([A-Za-z_]\w*)")


def statements(effect, fresh):
    """An action's effects as replay applies them. Timer effects only arm timers, and replay never arms one. A
    statement that sets a field this change adds leaves every record reading as if the field had always been there,
    and one that sets a field this change removes is reported once, as that field. Both are in `fresh`."""
    if effect is None:
        return ()
    items = effect if isinstance(effect, list) else [effect]
    out = []
    for e in items:
        if isinstance(e, dict) and e.get("type") in ("schedule", "schedule_at"):
            continue
        if not isinstance(e, str):
            out.append(json.dumps(e, sort_keys=True))
            continue
        target = TARGET.match(e)
        if e.strip().startswith("schedule(") or (target and target.group(1) in fresh):
            continue
        out.append(e.strip())
    return tuple(out)


def rules(spec, fresh):
    out = {}
    for action in spec.get("action", []):
        out.setdefault(action.get("name", ""), []).append((tuple(action.get("from", [])), action.get("to"), statements(action.get("effect"), fresh)))
    return out


def changes(path, old, new):
    """Each change in `new` that rewrites records `old` made, as `<path> <what>: <how>`."""
    found = []
    if new is None:
        return [(f"{path} spec", "the type is gone, and with it every record of it")]
    was, now = old.get("automaton", {}), new.get("automaton", {})
    if was.get("initial") != now.get("initial"):
        found.append((f"{path} initial", f"{was.get('initial')} becomes {now.get('initial')}"))
    for state in sorted(set(was.get("states", [])) - set(now.get("states", []))):
        found.append((f"{path} state {state}", "is gone"))
    had, has = {s.get("name") for s in old.get("state", [])}, {s.get("name") for s in new.get("state", [])}
    for field in sorted(had - has):
        found.append((f"{path} field {field}", "is gone, and with it what records hold in it"))
    before, after = rules(old, has ^ had), rules(new, has ^ had)
    for name, steps in before.items():
        if name not in after:
            # Its events keep the to they were stored with, so only effects are lost.
            if any(e for _, _, e in steps):
                found.append((f"{path} action {name}", f"is gone, and records lose {describe(steps)}"))
        elif not same(steps, after[name]):
            found.append((f"{path} action {name}", f"{describe(steps)} becomes {describe(after[name])}"))
    return found


def same(old, new):
    """An action replays the same when it keeps its to and effects. Its from may grow: an event that was valid stays
    valid. A place taken out of from counts as a change: an event that took it from there may no longer replay."""
    return len(old) == len(new) and all(t == nt and e == ne and set(f) <= set(nf) for (f, t, e), (nf, nt, ne) in zip(old, new))


def describe(steps):
    return "; ".join(f"from {list(f)} to {t or '(stays)'} with {list(e) or 'no effects'}" for f, t, e in steps)


def moved_to(head, name):
    """The spec at `head` whose automaton is `name`, for a file that was moved or renamed."""
    for path in git("ls-tree", "-r", "--name-only", head).splitlines():
        if SPEC.search(path):
            try:
                spec = parse(git("show", f"{head}:{path}"))
            except tomllib.TOMLDecodeError:
                continue
            if spec.get("automaton", {}).get("name") == name:
                return spec
    return None


def main(base, head, body):
    meant = {key.strip(): why.strip() for key, why in MARK.findall(body)}
    paths = [p for p in git("diff", "--name-only", "--no-renames", "--diff-filter=MD", base, head).splitlines() if SPEC.search(p)]
    failed = False
    for path in paths:
        try:
            old = parse(git("show", f"{base}:{path}"))
        except (subprocess.CalledProcessError, tomllib.TOMLDecodeError) as e:
            print(f"{path}: the base's spec could not be read ({e}), so nothing about it is checked")
            continue
        try:
            new = parse(git("show", f"{head}:{path}"))
        except subprocess.CalledProcessError:
            new = moved_to(head, old.get("automaton", {}).get("name"))
        except tomllib.TOMLDecodeError as e:
            print(f"::error file={path}::{path} does not parse as TOML: {e}")
            failed = True
            continue
        for key, how in changes(path, old, new):
            if key in meant:
                print(f"meant: {key}: {how} (because: {meant[key]})")
            else:
                print(f"::error file={path}::{key}: {how}. Records that went through it read differently once replayed. "
                      f"If that is meant, add the line `Replay intended: {key}: <why>` to the pull request's description.")
                failed = True
    print(f"{len(paths)} changed spec(s) checked against {base[:12]}" + (": replay-safe" if not failed else ""))
    return 1 if failed else 0


if __name__ == "__main__":
    body = open(sys.argv[3]).read() if len(sys.argv) > 3 else ""
    sys.exit(main(sys.argv[1], sys.argv[2], body))
