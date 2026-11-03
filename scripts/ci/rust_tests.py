#!/usr/bin/env python3
"""Runs `cargo test` in every Cargo workspace of the repository.

The root workspace holds crates/*. Every os-app module (os-apps/<app>/wasm/<module>) and every reference project
crate is a workspace of its own, which no `-p` from the root reaches. Workspaces are found on disk, so a new module is
tested without anyone naming it here.

usage: rust_tests.py root
       rust_tests.py others I/N [--list]

root        the root workspace, every member.
others I/N  the I-th of N parts of every other workspace. They share one target folder (target/workspaces, or
            $CARGO_TARGET_DIR), so a dependency they share is built once.
--list      prints the workspaces and runs nothing.

Each workspace's result goes to $GITHUB_STEP_SUMMARY when it is set, and the run fails if any workspace failed.
"""
import os
import re
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SKIP_DIRS = {"target", "node_modules", ".git"}
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def workspaces(root=ROOT):
    """Every directory below root whose Cargo.toml is a workspace root, relative to root and sorted."""
    found = []
    for path, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        if path != root and "Cargo.toml" in files:
            with open(os.path.join(path, "Cargo.toml")) as f:
                if re.search(r"^\[workspace\]", f.read(), re.M):
                    found.append(os.path.relpath(path, root))
    return sorted(found)


def run(workspace, target_dir=None):
    cmd = ["cargo", "test", "--workspace", "--no-fail-fast"]
    if os.path.exists(os.path.join(ROOT, workspace, "Cargo.lock")):
        cmd.append("--locked")
    env = dict(os.environ, **({"CARGO_TARGET_DIR": target_dir} if target_dir else {}))
    print(f"::group::{workspace}: {' '.join(cmd)}", flush=True)
    start = time.monotonic()
    proc = subprocess.Popen(cmd, cwd=os.path.join(ROOT, workspace), env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, errors="replace")
    failed, build_error = [], False
    for line in proc.stdout:
        sys.stdout.write(line)
        plain = ANSI.sub("", line).rstrip()
        m = re.match(r"^---- (\S+) stdout ----$", plain)
        if m:
            failed.append(m.group(1))
        if plain.startswith("error"):
            build_error = True
    code = proc.wait()
    print("::endgroup::", flush=True)
    status = "passed" if code == 0 else "failed" if failed else "build failed" if build_error else f"exit {code}"
    return {"workspace": workspace, "status": status, "seconds": time.monotonic() - start, "failed": failed}


def main(argv):
    if argv[:1] == ["root"]:
        selected, title, target_dir = ["."], "the root workspace", None
    elif argv[:1] == ["others"] and len(argv) > 1:
        index, count = (int(x) for x in argv[1].split("/"))
        selected = workspaces()[index - 1 :: count]
        title = f"other workspaces, part {index} of {count}"
        target_dir = os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target", "workspaces")
    else:
        sys.exit(__doc__)
    if "--list" in argv:
        print("\n".join(selected))
        return 0
    results = [run(w, target_dir) for w in selected]
    bad = [r for r in results if r["status"] != "passed"]
    lines = [f"### cargo test, {title}: {len(results) - len(bad)} of {len(results)} workspaces passed", "",
             "| Workspace | Result | Seconds | Failing tests |", "|---|---|---:|---|"]
    for r in sorted(results, key=lambda r: (r["status"] == "passed", -r["seconds"])):
        lines.append(f"| {r['workspace']} | {r['status']} | {r['seconds']:.0f} | {', '.join(r['failed'])} |")
        print(f"RESULT {r['workspace']} {r['status'].replace(' ', '-')} {r['seconds']:.0f}s {' '.join(r['failed'])}")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write("\n".join(lines) + "\n")
    for r in bad:
        print(f"::error::{r['workspace']}: {r['status']} {' '.join(r['failed'])}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
