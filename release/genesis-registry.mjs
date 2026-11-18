// Production Genesis, as CI publishes to it.
//
// A version is read as Temper's installer reads it: GET {genesis}/api/genesis/apps/{owner}/{app}/versions/{hash}/bundle,
// without a key. Genesis serves there the whole tree of that commit (temper genesis_install: `git ls-tree -r`), root
// app first, then its dependencies' closure.
//
// CI publishes (release/publish-apps.mjs; copied from katagami, whose ui/src/lib/line-registry.mjs it is) a release commit on top of the app repository's main, pushed over smart HTTP
// to /{owner}/{app}.git/git-receive-pack with the Genesis token as Basic auth and the tenant header, as `git push` does.
// Genesis may hold work of its own (on 1 Oct 2026 commons main carried Codex's private writing-style contributions,
// none of it on GitHub), so the publisher keeps its own branch there, `line`, holding the app exactly as it pushed it
// last, and main gets a three-way merge, file by file (`merge3`): base the `line` branch's files, theirs main's, ours
// what it pushes now. A file left as it was pushed last stays as main has it, a file main left alone takes the new one,
// and a file both changed differently stops the publish for a person. A repository with main but no `line` branch yet
// takes main as the base. The same push tags the version that serves the GitHub commit, `src/<commit>`, which the line
// reads (lib/line-apps.mjs app-state). Its objects go whole, in a pack with no deltas, which is all Genesis reads;
// objects Genesis already holds are recorded again unchanged, since an object's record is named by its bytes.
// owner/app@<commit> is served by the bundle route, for any commit of the repository, once Genesis has projected it.
// A commit that changes nothing of the app makes no version: the publish tags the version main holds.

import { createHash } from "node:crypto";
import { deflateSync } from "node:zlib";

/** Production's Genesis. */
export const PRODUCTION_GENESIS = "https://genesis-production-164d.up.railway.app";
/** A read Genesis has not answered in this long is a failed read. A bundle is the app and its whole dependency closure, modules included: 73 s for paw-channels on 17 Nov 2026. */
const READ_MS = 240_000;
/** A push: control's modules are ~9 MB, and Genesis records each object before it answers. */
const WRITE_MS = 180_000;
/** Who a release commit names as its author: the same name every Genesis release has carried. */
const IDENT = "TemperPaw release <41898282+github-actions[bot]@users.noreply.github.com>";
const NO_COMMIT = "0".repeat(40);

/** The root app's files at `version` as Genesis at `base` serves them: a Map of path to bytes, null when it serves no
 *  such version, or `{failed: status, reason}`. Read without a key, as the installer reads it, Genesis answers a bare 404
 *  for any version it will not serve; read with `token`, it says why (a dependency that resolves two ways), so then every
 *  refusal is `{failed, reason}`. */
export async function readBundle({ base, owner, app, version, token = "", fetchImpl = fetch }) {
  let reply;
  try {
    reply = await fetchImpl(`${base.replace(/\/+$/, "")}/api/genesis/apps/${owner}/${app}/versions/${version}/bundle`, {
      headers: { Accept: "application/json", "X-Tenant-Id": "default", ...(token ? { Authorization: `Basic ${Buffer.from(`${token}:`).toString("base64")}` } : {}) },
      signal: AbortSignal.timeout(READ_MS),
    });
  } catch {
    return { failed: 0, reason: "unreachable" };
  }
  if (reply.status === 404 && !token) return null;
  let bundle = null;
  let text = "";
  try {
    text = await reply.text();
    bundle = JSON.parse(text);
  } catch {
    bundle = null;
  }
  const root = reply.ok ? bundle?.apps?.[0] : null;
  if (!root || root.owner !== owner || root.name !== app || root.version_hash !== version || !Array.isArray(root.files)) {
    return { failed: reply.status, reason: String(bundle?.error ?? bundle?.message ?? text).slice(0, 300) };
  }
  return new Map(root.files.map((f) => [String(f.path), Buffer.from(String(f.content_base64 ?? ""), "base64")]));
}

/** The commit a ref of `owner/app` on Genesis at `base` names (`heads/main`, `tags/src/<commit>`): `{sha}`, "" when it
 *  has no such ref, or `{failed: status}`. Read without a key, as the line reads it. */
export async function readRef({ base, owner, app, ref, fetchImpl = fetch }) {
  let reply;
  try {
    reply = await fetchImpl(`${base.replace(/\/+$/, "")}/api/v3/repos/${owner}/${app}/git/ref/${ref}`, { headers: { Accept: "application/json", "X-Tenant-Id": "default" }, signal: AbortSignal.timeout(READ_MS) });
  } catch {
    return { failed: 0 };
  }
  if (reply.status === 404) return { sha: "" };
  let json = null;
  try {
    json = reply.ok ? await reply.json() : null;
  } catch {
    json = null;
  }
  const sha = json?.object?.type === "commit" ? String(json.object.sha ?? "") : "";
  return /^[0-9a-f]{40}$/.test(sha) ? { sha } : { failed: reply.status };
}

/** Whether two maps of path to bytes hold the same files. */
function sameFiles(a, b) {
  if (a.size !== b.size) return false;
  for (const [path, bytes] of a) if (!b.get(path)?.equals(bytes)) return false;
  return true;
}
const same = (a, b) => (a && b ? a.equals(b) : !a && !b);

/** The three-way merge of a published version: `theirs` (what Genesis main holds) with the change from `base` (what was
 *  published last) to `ours` (what is published now), each a Map of path to bytes. A file the change leaves alone stays
 *  as theirs; one theirs left alone takes the change (a deletion too); one both changed alike is kept; one both changed
 *  differently is a conflict. Module builds (wasm/) are never merged: ours holds the published build, which replaces theirs.
 *  Answers `{files}` or `{conflicts}`. */
export function merge3(base, ours, theirs) {
  const files = new Map();
  const conflicts = [];
  for (const path of [...new Set([...base.keys(), ...ours.keys(), ...theirs.keys()])].sort()) {
    const [b, o, t] = [base.get(path), ours.get(path), theirs.get(path)];
    let take;
    if (path.startsWith("wasm/")) take = o ?? t;
    else if (same(o, b)) take = t;
    else if (same(t, b) || same(t, o)) take = o;
    else {
      conflicts.push(path);
      continue;
    }
    if (take) files.set(path, take);
  }
  return conflicts.length ? { conflicts } : { files };
}

/** A git object: its type, its bytes and its id. */
function gitObject(type, body) {
  const head = Buffer.from(`${type} ${body.length}\0`);
  return { type, body, id: createHash("sha1").update(head).update(body).digest("hex") };
}

/** The tree of `files` (path to bytes) and every object under it, as git writes them: entries ordered by name, a
 *  folder's name compared with a "/" after it. */
export function gitTree(files) {
  const objects = [];
  const write = (folder) => {
    const entries = [...folder].map(([name, value]) => {
      if (Buffer.isBuffer(value)) {
        const blob = gitObject("blob", value);
        objects.push(blob);
        return { name, mode: "100644", id: blob.id, key: name };
      }
      return { name, mode: "40000", id: write(value), key: `${name}/` };
    });
    entries.sort((a, b) => Buffer.compare(Buffer.from(a.key), Buffer.from(b.key)));
    const tree = gitObject("tree", Buffer.concat(entries.flatMap((e) => [Buffer.from(`${e.mode} ${e.name}\0`), Buffer.from(e.id, "hex")])));
    objects.push(tree);
    return tree.id;
  };
  const root = new Map();
  for (const [path, bytes] of files) {
    const parts = path.split("/");
    let at = root;
    for (const part of parts.slice(0, -1)) {
      if (!at.has(part)) at.set(part, new Map());
      at = at.get(part);
    }
    at.set(parts.at(-1), bytes);
  }
  return { id: write(root), objects };
}

/** A pack of whole objects (git's version 2 format; Genesis reads no deltas). */
export function gitPack(objects) {
  const TYPES = { commit: 1, tree: 2, blob: 3 };
  const count = Buffer.alloc(4);
  count.writeUInt32BE(objects.length);
  const parts = [Buffer.from("PACK\0\0\0\x02", "latin1"), count];
  for (const o of objects) {
    let size = o.body.length;
    const head = [];
    let byte = (TYPES[o.type] << 4) | (size & 15);
    size = Math.floor(size / 16);
    while (size > 0) {
      head.push(byte | 0x80);
      byte = size & 0x7f;
      size = Math.floor(size / 128);
    }
    head.push(byte);
    parts.push(Buffer.from(head), deflateSync(o.body));
  }
  const body = Buffer.concat(parts);
  return Buffer.concat([body, createHash("sha1").update(body).digest()]);
}

/** A pkt-line. */
const pkt = (text) => {
  const payload = Buffer.from(text);
  return Buffer.concat([Buffer.from((payload.length + 4).toString(16).padStart(4, "0")), payload]);
};

/** The branch on Genesis that holds each app as it was published last: the base of the next merge. */
const LINE = "line";

/** Publishing to Genesis with `token()`, a GitToken that may push. Each answer is `{status, ...}`, an error `{error}`.
 *  - `publish(app, commit, files, {owner})`: the app's files as GitHub's `commit` has them, merged into main and kept
 *    on the line branch, and main tagged src/<commit>: `{outcome: published | already, ref}`, `ref` the tagged version.
 *  - `tag(owner, app, commit, version)`: `version` tagged src/<commit>, kept as it is when it is there already:
 *    `{outcome: tagged | already, ref}`.
 *  - `head(owner, app)`: `{main, line}`, the commits the two branches name ("" for none).
 *  - `filesAt(owner, app, version)`: a version's files, or `not_served` with Genesis's reason.
 *  - `ensureRepository(owner, app, description)` and `registerApp(owner, app, description)`: what a new app needs. */
export function createGenesisPublisher({ base = PRODUCTION_GENESIS, token, fetchImpl = fetch, now = () => Date.now() } = {}) {
  const ask = async (method, route, body, type) => {
    const auth = Buffer.from(`${token()}:`).toString("base64");
    const git = type === "application/x-git-receive-pack-request";
    try {
      const reply = await fetchImpl(`${base}${route}`, {
        method,
        headers: { Authorization: `Basic ${auth}`, "X-Tenant-Id": "default", ...(type ? { "Content-Type": type } : {}), Accept: git ? "application/x-git-receive-pack-result" : "application/json" },
        ...(body ? { body } : {}),
        signal: AbortSignal.timeout(git ? WRITE_MS : READ_MS),
      });
      return { status: reply.status, bytes: Buffer.from(await reply.arrayBuffer()) };
    } catch {
      return { status: 0, bytes: Buffer.alloc(0) };
    }
  };
  const ref = (owner, app, name) => readRef({ base, owner, app, ref: name, fetchImpl });
  const refused = (read) => ({ status: 502, error: "genesis_refused", genesis_status: read.failed });
  async function head(owner, app) {
    const main = await ref(owner, app, "heads/main");
    if (main.failed !== undefined) return refused(main);
    const line = await ref(owner, app, `heads/${LINE}`);
    if (line.failed !== undefined) return refused(line);
    return { status: 200, main: main.sha, line: line.sha };
  }
  /** A version's files, or `{error}` with Genesis's reason for not serving it (read with the token, so it gives one). */
  async function filesAt(owner, app, version) {
    const read = await readBundle({ base, owner, app, version, token: token(), fetchImpl });
    if (read.failed !== undefined) return { status: 502, error: "not_served", ref: `${owner}/${app}@${version}`, genesis_status: read.failed, reason: read.reason };
    return read;
  }

  async function tag(owner, app, commit, version) {
    if (!token()) return { status: 503, error: "not_configured" };
    const name = `tags/src/${commit}`;
    const made = await ask("POST", `/api/v3/repos/${owner}/${app}/git/refs`, JSON.stringify({ ref: `refs/${name}`, sha: version }), "application/json");
    if (made.status === 201) return { status: 200, outcome: "tagged", ref: `${owner}/${app}@${version}` };
    // A tag names what served its commit when it was made, and is never moved.
    const there = await ref(owner, app, name);
    if (there.sha) return { status: 200, outcome: "already", ref: `${owner}/${app}@${there.sha}` };
    return { status: 502, error: "tag_refused", genesis_status: made.status };
  }

  async function publish(app, commit, files, { owner }) {
    if (!token()) return { status: 503, error: "not_configured" };
    const { main, line, error, ...why } = await head(owner, app);
    if (error) return { error, ...why };
    const ours = files;
    let theirs = null;
    let pushed = null;
    let merged = ours;
    // A version Genesis will not serve (404: a dependency that resolves two ways) cannot be installed, so there is
    // nothing of it to keep: main is replaced as a first publish makes it, and the line branch as if it were not there.
    const unserved = (read) => read.error === "not_served" && read.genesis_status === 404;
    if (main) {
      theirs = await filesAt(owner, app, main);
      if (unserved(theirs)) theirs = null;
      else if (theirs.error) return theirs;
    }
    if (theirs) {
      pushed = line ? await filesAt(owner, app, line) : theirs;
      if (unserved(pushed)) pushed = theirs;
      else if (pushed.error) return pushed;
      const out = merge3(pushed, ours, theirs);
      if (out.conflicts) return { status: 409, error: "conflict", ref: `${owner}/${app}@${main}`, paths: out.conflicts.slice(0, 20) };
      merged = out.files;
    }
    // A commit that changes nothing of the app moves neither branch: main is tagged as it is.
    const moveMain = !theirs || !sameFiles(merged, theirs);
    const moveLine = !line || !pushed || !sameFiles(ours, pushed);
    if (!moveMain && !moveLine) {
      const tagged = await tag(owner, app, commit, main);
      return tagged.error ? tagged : { ...tagged, outcome: "already" };
    }

    const when = `${Math.floor(now() / 1000)} +0000`;
    const objects = new Map();
    const commitOf = (tree, parent, message) => {
      const { id, objects: own } = gitTree(tree);
      const made = gitObject("commit", Buffer.from(`tree ${id}\n${parent ? `parent ${parent}\n` : ""}author ${IDENT} ${when}\ncommitter ${IDENT} ${when}\n\n${message}`));
      for (const o of [...own, made]) objects.set(o.id, o);
      return made.id;
    };
    const updates = [];
    if (moveLine) updates.push({ old: line || NO_COMMIT, made: commitOf(ours, line, `${app} as published from GitHub ${commit}\n`), ref: `refs/heads/${LINE}` });
    const version = moveMain ? commitOf(merged, main, `TemperPaw release from GitHub (${commit}): snapshot of ${commit}\n`) : main;
    if (moveMain) updates.push({ old: main || NO_COMMIT, made: version, ref: "refs/heads/main" });
    // The tag moves with the branches, so a run that reads main after this push also finds what it serves.
    updates.push({ old: NO_COMMIT, made: version, ref: `refs/tags/src/${commit}` });
    const commands = updates.map((u, i) => pkt(`${u.old} ${u.made} ${u.ref}${i === 0 ? "\0report-status agent=temperpaw-ci" : ""}\n`));
    const body = Buffer.concat([...commands, Buffer.from("0000"), gitPack([...objects.values()])]);
    const sent = await ask("POST", `/${owner}/${app}.git/git-receive-pack`, body, "application/x-git-receive-pack-request");
    const report = sent.bytes.toString("utf8");
    if (sent.status !== 200 || !/unpack ok\n/.test(report) || !updates.every((u) => report.includes(`ok ${u.ref}\n`))) {
      // Genesis's report is about the push (a ref that moved, a pack it could not read), never the token.
      const reason = /ng refs\/\S+ ([^\n]{1,240})/.exec(report)?.[1] ?? /unpack (?!ok\n)([^\n]{1,240})/.exec(report)?.[1] ?? "";
      return { status: 502, error: "push_refused", genesis_status: sent.status, reason };
    }
    return { status: 200, outcome: moveMain ? "published" : "already", ref: `${owner}/${app}@${version}` };
  }
  const said = (sent) => sent.bytes.toString("utf8").slice(0, 200);
  /** The repository `owner/app` on Genesis, created (public, REST v3 `POST /api/v3/orgs/{owner}/repos`) when it has none: a
   *  first publish of a new app. Genesis allows that to a token with `admin:repos`, or `repo:write` on its own account. */
  async function ensureRepository(owner, app, description) {
    const there = await ask("GET", `/api/v3/repos/${owner}/${app}`);
    if (there.status === 200) return { status: 200, outcome: "repository exists" };
    if (there.status !== 404) return { status: 502, error: "genesis_refused", genesis_status: there.status };
    const made = await ask("POST", `/api/v3/orgs/${owner}/repos`, JSON.stringify({ name: app, description, visibility: "public" }), "application/json");
    if (made.status === 201) return { status: 200, outcome: "repository created" };
    return { status: 502, error: "repository_create_refused", genesis_status: made.status, reason: said(made), needs: "GENESIS_TOKEN with admin:repos (or repo:write as the account that owns the repository)" };
  }
  /** The App row of `owner/app` (Temper.Git.RegisterNewApp), which Genesis serves a bundle through, made when it has none. */
  async function registerApp(owner, app, description) {
    const route = `/tdata/Apps('app-${owner}-${app}')`;
    const had = await ask("GET", route);
    if (had.status === 200) return { status: 200, outcome: "app registered already" };
    if (had.status !== 404) return { status: 502, error: "app_lookup_refused", genesis_status: had.status, reason: said(had) };
    const body = JSON.stringify({ Name: app, RepositoryId: `rp-${owner}-${app}`, Description: description, Exports: "{}", Visibility: "public" });
    const made = await ask("POST", `${route}/Temper.Git.RegisterNewApp?await_integration=true`, body, "application/json");
    if (made.status >= 200 && made.status < 300) return { status: 200, outcome: "app registered" };
    return { status: 502, error: "app_register_refused", genesis_status: made.status, reason: said(made) };
  }
  return { publish, tag, head, filesAt, ensureRepository, registerApp };
}
