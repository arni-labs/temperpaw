#!/usr/bin/env python3
"""Builds and packages every os-app module, the way a release does.

Runs each os-app's build script: every os-apps/**/build.sh with no build.sh in a folder above it (a nested one, such
as dsf-twin's dsf_model_collect, is its app's to call). Each writes os-apps/<app>/wasm/<module>/<module>.wasm, which
the root workspace's tests load. Scripts are found on disk, so a new app's modules are built without anyone naming
them here.

Each script builds into its own modules' target folders, so a few run at once (JOBS, default 3, because each cargo
build already uses every core and a runner has 16 GB). A failing script's log is printed and the run fails.

usage: build_os_app_modules.py [--list]
"""
import concurrent.futures
import os
import subprocess
import sys
import tempfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def build_scripts(root=ROOT):
    found = []
    for path, dirs, files in os.walk(os.path.join(root, "os-apps")):
        dirs[:] = sorted(d for d in dirs if d not in {"target", "node_modules"})
        if "build.sh" in files:
            found.append(os.path.relpath(os.path.join(path, "build.sh"), root))
            dirs[:] = []  # a build.sh below this one is this script's to call
    return sorted(found)


def build(script, logs):
    log = os.path.join(logs, script.replace("/", "_") + ".log")
    start = time.monotonic()
    with open(log, "w") as out:
        code = subprocess.run(["bash", os.path.basename(script)], cwd=os.path.join(ROOT, os.path.dirname(script)),
                              stdout=out, stderr=subprocess.STDOUT).returncode
    return script, code, time.monotonic() - start, log


def main(argv):
    scripts = build_scripts()
    if "--list" in argv:
        print("\n".join(scripts))
        return 0
    logs = tempfile.mkdtemp(prefix="os-app-modules-")
    failed = []
    with concurrent.futures.ThreadPoolExecutor(int(os.environ.get("JOBS", "3"))) as pool:
        for script, code, seconds, log in pool.map(lambda s: build(s, logs), scripts):
            print(f"{'built' if code == 0 else 'FAILED'} {script} in {seconds:.0f}s", flush=True)
            if code:
                failed.append(script)
                print(f"::group::{script} log")
                print(open(log, errors="replace").read())
                print("::endgroup::", flush=True)
    for script in failed:
        print(f"::error::{script} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
