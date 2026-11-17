#!/usr/bin/env node
// Publishes this repository's Temper apps (each os-apps/<app> folder with an app.toml) to production Genesis from the
// commit a push to main tested, and tags in each app's Genesis repository the version that serves that commit,
// `src/<commit>`. ci.yml's publish job runs it once every job the push needed passed, after building every module
// (scripts/ci/build_os_app_modules.py). Modelled on katagami's release/publish-apps.mjs. Apps go in dependency order,
// and an app's dependency on another app of this repository (a bare name, or owner/app@<commit>) is rewritten to the
// version that app has for this same commit, so the versions a commit tags install together; a dependency on another
// repository's app is published as written. For each:
//  - src/<commit> is tagged already (a run again): nothing changes.
//  - a later main commit is tagged (this run is slower than a newer one's): nothing is published, so an older run
//    never overwrites a newer one, and the commit is tagged with what Genesis main holds.
//  - the app as the commit has it is what Genesis last received (its `line` branch): the commit is tagged with main.
//  - otherwise it is published (release/genesis-registry.mjs: merged into main with Genesis's own work and tagged, in
//    one push).
// Then the tagged version is read back as the installer reads it: a version Genesis will not serve fails the run at
// once with Genesis's reason.
// The app as a commit has it is the files git tracks in its folder, but its .wasm files, plus the modules the build
// left on disk (every .wasm under the folder but target/), which must include each module app.toml declares. A push
// refused because another run moved a ref first starts the app again.
//
// Usage: GENESIS_TOKEN=... node release/publish-apps.mjs <commit>   (from a checkout of <commit>, modules built)

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { PRODUCTION_GENESIS, createGenesisPublisher, readRef } from "./genesis-registry.mjs";

/** Every app of this repository is kept under this owner on Genesis. */
const OWNER = "temperpaw";
const APPS = "os-apps";
/** A push another run beat to a ref is tried again this often: each try reads Genesis afresh, so it ends tagged. */
const TRIES = 3;
/** What a build adds to a version: not compared, since a build's bytes differ by machine. */
const BUILT = /\.wasm$/;

const [commit] = process.argv.slice(2);
if (!/^[0-9a-f]{40}$/.test(commit ?? "")) throw new Error("usage: node release/publish-apps.mjs <commit>");
if (!process.env.GENESIS_TOKEN) throw new Error("GENESIS_TOKEN is not set");
const ROOT = execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim();
const git = (...args) => execFileSync("git", ["-C", ROOT, ...args], { maxBuffer: 1 << 30 });
const genesis = createGenesisPublisher({ token: () => process.env.GENESIS_TOKEN });

const DEPENDENCIES = /^dependencies\s*=\s*\[[^\]]*\]/m;
/** An app.toml dependency naming an app of this repository: its name, else "". `paw-fs`, `temperpaw/paw-fs@<hash>`. */
const named = (dep, apps) => {
  const [, owner = OWNER, name] = /^(?:([a-z0-9-]+)\/)?([a-z0-9-]+)(?:@[0-9a-f]{40})?$/.exec(dep) ?? [];
  return owner === OWNER && apps.has(name) ? name : "";
};
const entries = (list) => [...list.matchAll(/"([^"]*)"/g)].map((m) => m[1]);

/** The apps of this repository each of `app.toml`'s dependencies names. */
function dependenciesOf(toml, apps) {
  return entries(DEPENDENCIES.exec(toml)?.[0] ?? "").map((dep) => named(dep, apps)).filter(Boolean);
}

/** app.toml with each dependency on an app of this repository naming that app's version for this commit. */
function pinned(toml, apps, versions) {
  return toml.replace(DEPENDENCIES, (list) => list.replace(/"([^"]*)"/g, (whole, dep) => (named(dep, apps) ? `"${versions.get(named(dep, apps))}"` : whole)));
}

/** The app's files as `commit` has them, without its builds. */
function filesOf(app, apps, versions) {
  const files = new Map();
  const listing = git("ls-tree", "-r", "-z", commit, "--", `${APPS}/${app}/`).toString("utf8").split("\0").filter(Boolean);
  for (const line of listing) {
    const [meta, path] = line.split("\t");
    const [mode, type, id] = meta.split(" ");
    const inApp = path.slice(APPS.length + app.length + 2);
    if (type !== "blob" || mode === "120000" || BUILT.test(inApp)) continue;
    files.set(inApp, git("cat-file", "blob", id));
  }
  files.set("app.toml", Buffer.from(pinned(files.get("app.toml").toString("utf8"), apps, versions)));
  return files;
}

/** The app's module builds: every .wasm the build left under its folder, which must hold each declared module. */
function buildsOf(app, toml) {
  const builds = new Map();
  const walk = (dir) => {
    for (const entry of readdirSync(join(ROOT, APPS, app, dir), { withFileTypes: true })) {
      const path = dir ? `${dir}/${entry.name}` : entry.name;
      if (entry.isDirectory() && entry.name !== "target" && entry.name !== "node_modules") walk(path);
      else if (entry.isFile() && BUILT.test(entry.name)) builds.set(path, readFileSync(join(ROOT, APPS, app, path)));
    }
  };
  walk("");
  for (const [, name] of toml.matchAll(/^\[\[wasm_modules\]\]\s*\nname\s*=\s*"([a-z0-9_]+)"/gm)) {
    if (!builds.has(`wasm/${name}/${name}.wasm`) && !builds.has(`wasm/${name}.wasm`)) throw new Error(`${app}: module ${name} is declared in app.toml and was not built`);
  }
  return builds;
}

const unbuilt = (files) => new Map([...files].filter(([path]) => !BUILT.test(path)));
function same(a, b) {
  if (a.size !== b.size) return false;
  for (const [path, bytes] of a) if (!b.get(path)?.equals(bytes)) return false;
  return true;
}

/** The main commits after `commit`, newest first. */
function later() {
  git("fetch", "-q", "origin", "main");
  return git("rev-list", `${commit}..origin/main`).toString("utf8").split("\n").filter(Boolean);
}

/** One app, once: `{outcome, ref}` or `{error}`. */
async function ship(app, apps, after, versions) {
  const tag = (name) => readRef({ base: PRODUCTION_GENESIS, owner: OWNER, app, ref: `tags/src/${name}` });
  const mine = await tag(commit);
  if (mine.failed !== undefined) return { error: "genesis_refused", genesis_status: mine.failed };
  if (mine.sha) return { outcome: "tagged already", ref: `${OWNER}/${app}@${mine.sha}` };
  const head = await genesis.head(OWNER, app);
  if (head.error) return head;
  for (const newer of after) {
    const theirs = await tag(newer);
    if (theirs.failed !== undefined) return { error: "genesis_refused", genesis_status: theirs.failed };
    if (theirs.sha) return { ...(await genesis.tag(OWNER, app, commit, head.main)), why: `${newer} is published already` };
  }
  const files = filesOf(app, apps, versions);
  const last = head.line || head.main;
  if (last) {
    // A version Genesis will not serve (404) is not what this commit has, whatever its files: it is published over.
    const was = await genesis.filesAt(OWNER, app, last);
    if (was.error && was.genesis_status !== 404) return was;
    if (!was.error && same(unbuilt(was), files)) return { ...(await genesis.tag(OWNER, app, commit, head.main)), why: "unchanged" };
  }
  for (const [path, bytes] of buildsOf(app, readFileSync(join(ROOT, APPS, app, "app.toml"), "utf8"))) files.set(path, bytes);
  return genesis.publish(app, commit, files, { owner: OWNER });
}

/** The repository's apps at `commit`, each after the apps of this repository it depends on. */
function inOrder() {
  const found = git("ls-tree", "--name-only", commit, `${APPS}/`).toString("utf8").split("\n").map((p) => p.slice(APPS.length + 1));
  const apps = new Set(found.filter((dir) => dir && existsSync(join(ROOT, APPS, dir, "app.toml"))));
  if (!apps.size) throw new Error(`no app folder at ${commit}`);
  const needs = new Map([...apps].map((app) => [app, dependenciesOf(git("show", `${commit}:${APPS}/${app}/app.toml`).toString("utf8"), apps)]));
  const order = [];
  const visit = (app, path) => {
    if (order.includes(app)) return;
    if (path.includes(app)) throw new Error(`apps depend on each other in a circle: ${[...path, app].join(" -> ")}`);
    for (const dep of needs.get(app)) visit(dep, [...path, app]);
    order.push(app);
  };
  for (const app of apps) visit(app, []);
  return { apps, order, needs };
}

const { apps, order, needs } = inOrder();
const after = later();
/** Each app's version for this commit, `owner/app@<version>`, once tagged. */
const versions = new Map();
let failed = 0;
for (const app of order) {
  const missing = needs.get(app).filter((dep) => !versions.has(dep));
  let out;
  if (missing.length) {
    out = { error: "dependency_failed", dependencies: missing };
  } else {
    out = await ship(app, apps, after, versions);
    for (let tries = 1; out.error === "push_refused" && tries < TRIES; tries += 1) {
      console.log(`${app}: push_refused ${out.reason ?? ""}; again`);
      out = await ship(app, apps, after, versions);
    }
  }
  // Proof the version installs: Genesis serves it, read with the token so a refusal says why.
  if (!out.error) {
    const read = await genesis.filesAt(OWNER, app, out.ref.slice(out.ref.indexOf("@") + 1));
    if (read.error) out = { ...read, tagged: out.ref };
    else versions.set(app, out.ref);
  }
  const { status: _, ...said } = out;
  console.log(`${app}: ${JSON.stringify(said)}`);
  if (out.error) failed += 1;
}
process.exit(failed ? 1 : 0);
