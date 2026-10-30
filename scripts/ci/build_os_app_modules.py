#!/usr/bin/env python3
"""Builds and packages every os-app module, the way a release does.

Runs each os-app's build script: every os-apps/**/build.sh with no build.sh in a folder above it (a nested one, such
as dsf-twin's dsf_model_collect, is its app's to call). Each writes os-apps/<app>/wasm/<module>/<module>.wasm, which
the root workspace's tests load. Scripts are found on disk, so a new app's modules are built without anyone naming
them here.

With --reuse (CI), a script whose inputs have not changed since its last build is skipped. Its inputs are the files
git tracks in its app's folder, in every app folder its crates reach through path dependencies, and
os-apps/wasm-build-env.sh; each build records them as a stamp in ~/.cache/temperpaw-os-app-modules. The workflow
restores the packaged modules and the stamps from main's latest cache, so a pull request rebuilds only the apps it
changes. Without --reuse (the nightly run, or anyone building by hand, whose edits git does not track yet) every
script builds, so dependencies a module does not lock move on.

Each script builds into its own modules' target folders, so a few run at once (JOBS, default 3, because each cargo
build already uses every core). A failing script's log is printed and the run fails.

usage: build_os_app_modules.py [--list | --reuse]
"""
import concurrent.futures
import hashlib
import os
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
STAMPS = os.path.expanduser("~/.cache/temperpaw-os-app-modules")
SKIP_DIRS = {"target", "node_modules", ".git"}


def build_scripts(root=ROOT):
    found = []
    for path, dirs, files in os.walk(os.path.join(root, "os-apps")):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        if "build.sh" in files:
            found.append(os.path.relpath(os.path.join(path, "build.sh"), root))
            dirs[:] = []  # a build.sh below this one is this script's to call
    return sorted(found)


def tracked(folder):
    """(path, blob) for every file git tracks under folder, but the packaged modules."""
    out = subprocess.run(["git", "-C", ROOT, "ls-files", "-s", "-z", "--", folder], check=True, capture_output=True,
                         text=True).stdout
    entries = [line.split("\t", 1) for line in out.split("\0") if line]
    return [(path, meta.split()[1]) for meta, path in entries if not path.endswith(".wasm")]


def path_dependencies(manifest):
    with open(os.path.join(ROOT, manifest), "rb") as f:
        data = tomllib.load(f)
    tables = [data.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    for target in data.get("target", {}).values():
        tables += [target.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    for table in tables:
        for dep in table.values():
            if isinstance(dep, dict) and "path" in dep:
                yield os.path.normpath(os.path.join(os.path.dirname(manifest), dep["path"]))


def app_of(path):
    return "/".join(path.split("/")[:2]) if path.startswith("os-apps/") else path


def inputs(script):
    """What the script's build reads, as git tracks it: its app folder, every app folder its crates reach through
    path dependencies, and the shared build env."""
    folders, queue = set(), [app_of(script)]
    while queue:
        folder = queue.pop()
        if folder in folders:
            continue
        folders.add(folder)
        for path, _ in tracked(folder):
            if os.path.basename(path) == "Cargo.toml":
                queue.extend(app_of(dep) for dep in path_dependencies(path))
    return sorted(set(tracked("os-apps/wasm-build-env.sh")).union(*(tracked(f) for f in folders)))


def stamp(script):
    return hashlib.sha256("".join(f"{path} {blob}\n" for path, blob in inputs(script)).encode()).hexdigest()


def stamp_file(script):
    return os.path.join(STAMPS, script.replace("/", "_") + ".stamp")


def build(script, want, logs, reuse):
    if reuse and os.path.exists(stamp_file(script)) and open(stamp_file(script)).read() == want:
        return script, "unchanged", 0.0, None
    log = os.path.join(logs, script.replace("/", "_") + ".log")
    start = time.monotonic()
    with open(log, "w") as out:
        code = subprocess.run(["bash", os.path.basename(script)], cwd=os.path.join(ROOT, os.path.dirname(script)),
                              stdout=out, stderr=subprocess.STDOUT).returncode
    if code == 0:
        os.makedirs(STAMPS, exist_ok=True)
        with open(stamp_file(script), "w") as f:
            f.write(want)
    elif os.path.exists(stamp_file(script)):
        os.remove(stamp_file(script))
    return script, "built" if code == 0 else "FAILED", time.monotonic() - start, log


def main(argv):
    scripts = build_scripts()
    if "--list" in argv:
        print("\n".join(scripts))
        return 0
    reuse = "--reuse" in argv
    wants = {script: stamp(script) for script in scripts}  # before any build adds files to a folder
    logs = tempfile.mkdtemp(prefix="os-app-modules-")
    failed = []
    with concurrent.futures.ThreadPoolExecutor(int(os.environ.get("JOBS", "3"))) as pool:
        for script, result, seconds, log in pool.map(lambda s: build(s, wants[s], logs, reuse), scripts):
            print(f"{result} {script}" + (f" in {seconds:.0f}s" if log else ""), flush=True)
            if result == "FAILED":
                failed.append(script)
                print(f"::group::{script} log")
                print(open(log, errors="replace").read())
                print("::endgroup::", flush=True)
    for script in failed:
        print(f"::error::{script} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
