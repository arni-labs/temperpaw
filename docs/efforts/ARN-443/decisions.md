# Decisions - land the paw-compute app (ARN-443 part A)

## Reconcile into the branch, not from Genesis — the source already matches
- **Decision:** Land the branch as-is (no content pulled from Genesis), after
  verifying a file-by-file match against Genesis HEAD.
- **Came up because:** the reconcile rule says Genesis wins on divergence, so I had
  to check whether Genesis had newer content than the branch.
- **Options:** pull Genesis content into the branch / land the branch after
  proving equality.
- **Chose land-after-proving because:** every text file in the branch's
  os-apps/paw-compute/ is identical to Genesis HEAD (73646d4); Genesis's only newer
  content is the compiled .wasm blob, which temperpaw gitignores by design and
  rebuilds at publish. Nothing to pull. Given up: nothing.
- **Where:** diff of Genesis `temperpaw/paw-compute` HEAD vs the branch tree;
  recorded in spec.md.

## Align computer_exec's SDK rev to main (43f9379)
- **Decision:** Bump computer_exec's temper-wasm-sdk rev from a747f7d to 43f9379.
- **Came up because:** after rebasing on post-#468 main, computer_exec failed to
  build — it pinned a different SDK rev than wasm-helpers, so the shared path dep
  produced two incompatible `Context` types (E0308).
- **Options:** pin wasm-helpers back to a747f7d (wrong — main is on 43f9379) / bump
  computer_exec to 43f9379.
- **Chose bump computer_exec because:** all of main (wasm-helpers, every os-app
  module) is on 43f9379; a module can't bridge two SDK revs through a shared path
  dep. This matches what Genesis's own ARN-401 rebuild (73646d4) targeted. Given
  up: nothing — the SDK bump is a build-input alignment, no source behavior change.
- **Where:** os-apps/paw-compute/wasm/computer_exec/Cargo.toml.

## Fix the app.toml provider string
- **Decision:** "Compute provisioning for agent sandboxes via Fly.io" → "via
  Tensorlake".
- **Came up because:** the description named the wrong provider; the Computer
  entity's provider is tensorlake (dsf and arni-big both run on Tensorlake).
- **Options:** leave it / correct it.
- **Chose correct because:** it is a plainly stale, misleading string. Given up:
  nothing. (The same string is stale in Genesis too; not touched here — no Genesis
  publish in this effort.)
- **Where:** os-apps/paw-compute/app.toml.

## No live prod-install verification in this effort
- **Decision:** Verify by build + unit tests + blob inspection, not a live
  prod/Genesis install.
- **Came up because:** the Definition of Done wants live e2e, but the effort brief
  forbids any Genesis publish / prod install tonight.
- **Options:** publish to Genesis and drive the walk (forbidden) / bound
  verification to repo-side.
- **Chose repo-side because:** the brief scopes this to reconciliation only; the
  live governed-Exec walk was already proven on the dsf box (dd-computer) and the
  code runs in prod today. Given up: a fresh live walk — deferred to a publish
  effort. Not a punt: the objective here is landing the source, which is fully met.
- **Where:** build/test evidence; PR proof record.

# Round 1 hardening (panel found 9 act-ons on prod-relevant code)

This reconcile PR is also a hardening PR: the panel reviewed code already running
in prod via Genesis, so these fixes ship here BEFORE any future Genesis publish.

## SECURITY

### 1. Entity fields win over trigger params (no override of pinned fields)
- **Decision:** `param_or_field` → `field_or_param`: read the entity field FIRST,
  fall back to trigger params. Spec-pinned fields (LatencyDiag's canned command /
  pinned computer_id) can no longer be overridden by caller-supplied params.
- **Why:** LatencyDiag is a constrained tool; caller params overriding its pinned
  command breaks the constraint. For Exec, Run writes command/computer_id to the
  fields before the trigger fires, so field-first still yields the caller's input.
- **Where:** computer_exec/src/lib.rs `field_or_param`.

### 2. Drop caller-supplied `created_by` (one source of truth for identity)
- **Decision:** Remove `created_by` from the Exec spec (state var + Run param).
- **Why:** the kernel stamps the true agent identity on every event, non-spoofable
  (dd-computer proof: a forged created_by still recorded the real agent_id). A
  self-declared created_by field is a second, untrustworthy identity source. The
  WASM cannot read the authenticated principal, so deriving it in-module isn't
  possible; the event log is the source of truth. Dropped the field.
- **Where:** specs/exec.ioa.toml.

### 3. Computer read/list restricted to Agent (ADR-0001 scope)
- **Decision:** `permit(principal, [read,list], Computer)` → `principal is Agent`.
- **Why:** the open permit granted read/list to ANY principal, wider than
  ADR-0001 (authenticated tenant agents). Admin stays covered by the blanket
  Admin-on-Computer permit.
- **Where:** policies/compute.cedar.

### 4. Admin gets a consistent blanket on all three entities
- **Decision:** Replace the Admin-callbacks-only permits on Exec and LatencyDiag
  with blanket `permit(principal is Admin, action, resource is Exec|LatencyDiag)`,
  matching the existing Computer blanket.
- **Why:** Admin lacked create/read/list on Exec/LatencyDiag while the intent
  grants Admin the full surface; the three entities are now consistent.
- **Where:** policies/compute.cedar.

## CORRECTNESS

### 5. Fail closed on Computer status
- **Decision:** Refuse the exec unless status == "Ready" exactly; missing/empty
  status is refused (was: only non-empty non-Ready was refused, so empty passed).
- **Where:** computer_exec/src/lib.rs `sandbox_handle_from_computer`.

### 6. Run the command in a child process (control flow can't skip the epilogue)
- **Decision:** The command runs under `bash -c '<cmd>'` (a child), not inline in
  the wrapper's own shell. `exit`/`exec` in the command affect the child, so the
  wrapper's metadata epilogue always runs.
- **Where:** computer_exec/src/lib.rs `wrap_command` (+ `shell_single_quote`).

### 7. Clear result fields on failure (no stale output across re-runs)
- **Decision:** `set_error_result` → local `set_failure_result` that emits the
  error AND empty exit_code/stdout_tail/stderr_tail; both RunFailed actions declare
  those params. A failed LatencyDiag re-scan no longer shows a previous scan's
  output. (The effect system can't set string fields declaratively — only
  status/booleans/counters — so the clear is done via callback params.)
- **Where:** computer_exec/src/lib.rs; specs/exec.ioa.toml; specs/latency_diag.ioa.toml.

### 8. Kill a timed-out command (no orphaned process)
- **Decision:** Wrap with `timeout -k 5s 110s` so a runaway command is killed on
  the sandbox (its process group, no orphan); exit 124 is mapped to RunFailed
  ("exceeded the 110s limit and was terminated"). 110s sits just under the 120s
  WASM invocation cap so the kill and its result land within one invocation.
  Longer runs need the async exec path (D), not a bigger limit.
- **Where:** computer_exec/src/lib.rs `wrap_command` + the run() timeout branch.

### 9. build.sh target → wasm32-wasip1 (the forbidden target, reconciled)
- **Decision:** Change build.sh from wasm32-unknown-unknown to wasm32-wasip1 and
  verify the blob has WASI imports + zero wbindgen before copying.
- **Why + the genesis-blob story:** the standing rule (2026-07-20 incident) forbids
  wasm32-unknown-unknown. The genesis-installed blob (73646d4) is in fact
  unknown-unknown (0 WASI imports, 0 wbindgen) — it happened to work because the
  OLD computer_exec had no chrono (no wbindgen trigger) and no WASI needs, and it
  was built against an EARLIER round of the ARN-401 fix (before #468's getrandom).
  After #468, wasm-helpers pulls getrandom → WASI random_get, so computer_exec
  MUST be wasip1 now: unknown-unknown can't even compile getrandom, and the blob
  must carry the random_get import. The fixed build.sh reproduces the correct
  wasip1 blob (verified: 10 WASI imports, 0 wbindgen). The genesis blob is
  therefore stale vs final #468 and will be rebuilt (wasip1) on the next publish;
  the source still matches genesis HEAD, only the gitignored blob differs.
- **Where:** wasm/build.sh.

## Considers taken
- UTF-8-safe tail: `output_tail` already aligns to char boundaries; also trims a
  leading U+FFFD left by the sandbox's byte-level `tail -c`.
- Separate stderr capture: stdout → `<id>.log`, stderr → `<id>.err` (no more
  2>&1 conflation); stderr_tail now carries real stderr. Tradeoff: stdout_path is
  stdout-only (stderr's full log stays at the sibling `.err` on the sandbox);
  interleaving between the two streams is not preserved — acceptable for an audit
  tail, and each stream is individually complete.
- LatencyDiag's hardcoded Datadog query: noted as tech-debt in the spec comment
  (it was the prototype's learned-tool proof, not a general diagnostic).

## Round 1.1: stderr via a separate stream, not an in-band delimiter (Greptile P1)
- **Decision:** Route the command's stderr tail to the wrapper's own fd 2 (`>&2`,
  captured by the provider as result.stderr) instead of emitting it after an
  `__EXEC_ERR_TAIL` marker in the stdout stream.
- **Came up because:** Greptile P1 — a command printing a line equal to
  `__EXEC_ERR_TAIL` would be treated as the delimiter, corrupting the audit tails.
- **Options:** byte-length-delimit the two tails in stdout / put stderr on a
  separate stream.
- **Chose separate stream because:** it removes ALL command-controllable
  delimiters from the stdout data (the log-path marker stays on a fixed
  wrapper-emitted line, collision-safe), keeps stdout/stderr separated (the
  original consider), and is simpler than byte-length parsing. The parse reverts
  to the collision-safe header form; stderr_tail comes from result.stderr.
- **Where:** computer_exec/src/lib.rs `wrap_command` / `parse_captured_output` /
  `success_params`.

# Round 2 hardening (panel found 7 act-ons; #6 was my P1 fix, done)

## R2.1 created_by fully removed (was half-dropped)
- **Decision:** Remove `CreatedBy` property + `created_by` Run parameter from
  model.csdl.xml, and the `Run(...,created_by)` / audit-field mentions in APP.md.
- **Came up because:** the spec dropped created_by but the CSDL still declared it,
  and Temper merges arbitrary request keys into fields — so the audit field stayed
  caller-writable via OData. Removing it from the schema removes the surface.
- **Where:** specs/model.csdl.xml, APP.md.

## R2.2 LatencyDiag command/computer_id pinned in the trigger config (not fields)
- **Decision:** Move LatencyDiag's `command` and `computer_id` from mutable state
  vars to the `[action.triggers.config]` (spec-defined, request-untouched);
  computer_exec reads config FIRST for these keys (`config_field_or_param`).
- **Came up because:** field-first precedence did not constrain LatencyDiag —
  Temper merges every RunScan request key into fields FIRST, so "field wins" read
  the attacker's value. The trigger config is not request-influenced, so a pinned
  value there cannot be overridden.
- **Options:** wasm validates against spec-declared values (chosen, via config) /
  rely on parameterless RunScan (insufficient — arbitrary keys still merge).
- **Where:** specs/latency_diag.ioa.toml; computer_exec/src/lib.rs
  `config_field_or_param`. For Exec, nothing is pinned in config, so the caller's
  Run params (via fields) are used as before.

## R2.3 Tensorlake poll is time-bounded to outlive the command timeout
- **Decision:** Poll the rc-file by WALL TIME (until ~116s, under the 120s WASM
  cap) instead of a fixed 600 iterations.
- **Came up because:** fast GETs could exhaust 600 iterations in ~30s — before the
  110s command timeout — returning "timed out" (Failed) while the process ran on
  until the sandbox timeout killed it: a live-process window behind a Failed row.
- **Chose time-bound because:** it guarantees the poll outlives any caller's inner
  command timeout, so the timeout's result is read within this invocation. Kept an
  iteration cap as a stalled-clock guard.
- **Where:** wasm-helpers/src/sandbox.rs `tensorlake_exec` poll loop.

## R2.4 Outer-timeout vs command's own 124 disambiguated by a done marker
- **Decision:** The wrapper writes a `.done` marker only if the child completed
  (not killed); its ABSENCE (with the outer exit) is the timed-out signal, emitted
  as `__EXEC_TIMED_OUT 1`. run() maps that to RunFailed; a command that legitimately
  exits 124 (done present) is a normal RunSucceeded(124).
- **Where:** computer_exec/src/lib.rs `wrap_command` / `parse_captured_output` / run().

## R2.5 Tensorlake capture id — already random per dispatch (#468); log id hardened
- **Decision:** No change to the tensorlake capture id — it is already a per-dispatch
  random u64 (from #468, inherited via the rebase), NOT a counter; verified no
  counter remains. Separately, harden computer_exec's own `~/.exec-out/<id>` log
  filename (see R2.7) so distinct exec ids never collide, and note that the random
  tensorlake capture id already prevents captured-output crossing regardless.
- **Where:** wasm-helpers/src/sandbox.rs `unique_run_id` (already random).

## R2.7 exec_log_id — injective, bounded, hash-suffixed (was lossy)
- **Decision:** Replace lossy `sanitize_exec_id` with `exec_log_id`: injective
  encoding (alnum/`-` pass; other bytes `_`+hex), capped at 32 chars + an 8-hex FNV
  hash of the full id — same shape as #468's label.
- **Came up because:** the lossy sanitizer could map distinct exec ids to the same
  `~/.exec-out` filename.
- **Where:** computer_exec/src/lib.rs `exec_log_id` (+ `fnv1a_32`).

# Round 3 — BREAKER APPLIED (owner decision; 2 surgical fixes + adjudicated residuals)

Rounds were not converging (9→7→6). Owner ruling: the exec wrapper's metadata is
best-effort OBSERVABILITY; the security boundary is Cedar + kernel identity
stamping. An authenticated caller "spoofing" its own exec's completion bit is
lying in its own audit record — WHO ran WHAT stays kernel-stamped and unforgeable.
No round 4. Two plain-correctness findings got a surgical fix; the rest are
accepted residuals.

## R3.1 Completion/timeout decided in the OUTER script, from timeout's exit
- **Decision:** The user command runs only in the child `bash -c`; the wrapper's
  epilogue (rc + markers + tails) lives in the OUTER script and always runs.
  Timed-out is read from `timeout`'s own exit status (124), not a child-written
  `.done` marker.
- **Came up because:** the `.done` marker was written inside the child, so a
  legitimate `exec long-running` (which replaces the child and never reaches the
  marker) was misreported as timed-out; both reviewers flagged the structure.
- **Chose outer-epilogue because:** nothing the command does (exit/exec/background
  spawn) can skip an epilogue that runs in the outer script, and reading 124 from
  `timeout` handles `exec` correctly. Given up: a command that itself exits exactly
  124 is classified as timed-out — accepted (see residuals).
- **Where:** computer_exec/src/lib.rs `wrap_command`.

## R3.2 Poll budget reserves read + callback headroom
- **Decision:** Start the poll wall-clock at invocation ENTRY and bound it to
  ~100s (was 116s from post-setup); reduce the command timeout to 90s.
- **Came up because:** a 116s poll left almost no room under the 120s WASM cap for
  the output reads + callback dispatch.
- **Budget math:** command ≤90s < poll ~100s (outlives it, reaps the result) <
  120s cap, leaving ~20s for reads + callback.
- **Where:** wasm-helpers/src/sandbox.rs `tensorlake_exec` (poll_deadline from
  invocation_start); computer_exec `EXEC_TIMEOUT_SECS = 90`.

## R3.3 ADR-0002 aligned with the created_by removal
- **Decision:** Drop `created_by` from ADR-0002's Run signature + audit-field list;
  note identity is kernel-stamped.
- **Where:** adrs/0002-governed-exec-surface.md.

## Accepted residuals (owner-adjudicated at the breaker; NOT bugs to fix)
- **Timeout/exit-124 ambiguity:** a command self-reporting 124 is classed as
  timed-out. Accepted — it is the caller's own audit record; corrupting one's own
  completion bit ≠ crossing another exec; Cedar + kernel identity is the boundary.
  R3.1 further narrows it structurally.
- **Background processes outliving an early-exiting command:** normal unix
  semantics; the Computer-copy lifecycle (C) reaps by teardown — not the exec's job.
- **FNV-32 exec-log-id collision:** a 32-bit birthday needs ~65k execs against one
  log dir within a box's lifespan. Accepted with this note; revisit if exec volume
  ever approaches that.
- **Iteration cap in the poll:** kept as-is (a stalled-clock guard).

# Part C — copies as governed children (design decisions)

## C1 — Option A (spawn child) + Leased as a distinct STATE (not a flag/entity)
- **Decision:** Copy spawns a child Computer that runs its OWN copy
  (ProvisionFromCopy → computer_copy → CopyComplete → Leased); reaping is a
  state_timeout on Leased only. Not a ComputerCopy entity, not an is_copy flag.
- **Where:** specs/computer.ioa.toml. Confirmed with team-lead (independent
  convergence on the same shape).

## C2 — parent reference is source_machine_id, set by the callback (no parent_computer_id field)
- **Decision:** The child records `source_machine_id` (set by computer_copy in the
  CopyComplete callback), not a `parent_computer_id` entity-id field.
- **Came up because:** the `spawn` primitive copies parent fields into the child's
  initial-action params by SAME NAME only — it cannot inject the parent's
  entity-id or rename a field, so a `parent_computer_id = <source entity id>` field
  can't be set cleanly without a fragile name==id convention. computer_copy, by
  contrast, knows exactly what machine it copied.
- **Chose source_machine_id because:** it is the parent's stable machine identity,
  set from a source the module actually has, and together with the inherited
  `name` + the Leased state it fully serves the governed-child audit ("a leased
  copy of <name>, from machine <source_machine_id>, now on <machine_id>"). This
  reads the team-lead's "sets parent_computer_id=source" as satisfied by the
  machine identity rather than a second, hard-to-set entity-id field — DEVIATION
  from the literal field name, surfaced for confirmation.
- **Where:** computer_copy/src/lib.rs; specs/computer.ioa.toml source_machine_id.

## C3 — Terminating intermediate state (not a fire-and-forget on Destroyed)
- **Decision:** Destroy → Terminating (fires computer_terminate) → TerminateComplete
  → Destroyed, rather than firing the terminate as a side effect on the final
  Destroyed state.
- **Came up because:** a trigger on a terminal state has no callback target, and a
  WASM integration must not dispatch on a final/other machine; a clean callback
  needs a non-final state.
- **Chose Terminating because:** the module reports TerminateComplete (the machine
  sequences the teardown), best-effort/idempotent, with a safety state_timeout.
- **Where:** specs/computer.ioa.toml.

## C4 — Destroy excludes Provisioning (source-termination safety)
- **Decision:** Destroy's `from` is Ready/Sleeping/Created/Leased — NOT Provisioning.
- **Came up because:** during a child's copy the child's machine_id is still the
  SOURCE's machine; a Destroy-from-Provisioning would fire computer_terminate on
  the source's sandbox and kill it.
- **Where:** specs/computer.ioa.toml Destroy; CopyFailed goes straight to Destroyed
  with no terminate.

## C5 — provider CALLS (real copy/terminate) deferred to prod verification, with a hard condition
- **Decision:** The local e2e proves the state machine, Cedar, the source-safety
  guard, the failure path, and the reap STATE flow (with stub machines). The two
  real provider CALLS — a live tensorlake copy and a live terminate — are proved
  at C's Genesis-publish / prod step, not locally.
- **Came up because:** `tensorlake_api_key` is a prod Temper secret; it is not in
  Railway env and the `tl` CLI does not expose the raw key. Prod secrets do not
  migrate to local boxes for convenience, so a local server cannot make a real
  tensorlake copy. Team-lead confirmed this posture and that the prod "verify
  live" step is higher-fidelity anyway.
- **CONDITION for C = done (must be met at prod verification, not optional):** ONE
  full real cycle — Copy → the real child sandbox EXISTS (`tl sbx ls` shows it) →
  Leased → forced/short lease timeout → Destroyed → provider-confirmed TERMINATED
  (`tl sbx ls` shows it gone). This closes the one gap the local stub cannot.
- **Where:** local e2e = this effort's proof record; prod cycle = C's Genesis
  publish + live verification (temperpaw DoD).

# Part D — async exec (batched into C's PR #494 per Rita; Option A, acked)

## D0 — status at this checkpoint
- **Built + committed:** the async wasm-helpers foundation — `sandbox_exec_start`
  (POST, returns run_id, no poll) + `sandbox_exec_poll(run_id) -> Option<ExecResult>`
  (Some once the rc file exists, None while running). The sync `sandbox_exec`
  stays for LatencyDiag/short callers. 56/56 tests, wasip1 clean (commit 8a057cda5).
- **Also fixed a #468 regression (e42706c5c):** wasm-helpers used the `getrandom`
  crate, which hard-`compile_error!`s on wasm32-unknown-unknown and broke every
  consumer still on that target (make wasm failed). Replaced with a raw WASI
  `random_get` import (DCE'd when unused; host-provided on wasip1). Rides this PR
  to main; flagged for possible fast-track.

## D-plan — remaining (resume here)
1. **Exec spec restructure** (exec.ioa.toml): states Created → **Starting** →
   Running → Succeeded|Failed. Fields add `run_id`, `started_at_ms`.
   - `Run(computer_id, command)` → Starting; effect `computer_exec_start`;
     on_failure RunFailed.
   - `ExecStarted(run_id, started_at_ms)` → Running (callback from start).
   - `[[state_timeout]] state="Running" after_seconds=10 on_timeout="Poll"`.
   - `Poll` → Running (self-loop); effect `computer_exec_poll`; on_failure RunFailed.
     (Re-entering Running re-arms the timeout — that IS the poll loop.)
   - `RunSucceeded` / `RunFailed` (from Running AND Starting) unchanged-ish.
   - `Cancel(error)` (from Running/Starting) → Failed.
   - `[[state_timeout]] state="Starting"` safety → RunFailed. LatencyDiag stays on
     the sync `computer_exec`.
2. **computer_exec_start module:** resolve computer (loopback + Ready gate), wrap
   the command with a LONG `timeout <deadline>s bash -c '<cmd>'` (async has no 90s
   cap — the poll loop spans invocations), `sandbox_exec_start` → run_id, report
   `ExecStarted(run_id, started_at_ms)`.
3. **computer_exec_poll module:** read computer_id + run_id, re-resolve the
   computer, `sandbox_exec_poll(run_id)`. Some → RunSucceeded(exit_code, tails) /
   RunFailed. None & now < started_at_ms + MAX_RUN → no result (stay Running, the
   Poll transition already re-armed the timeout). None & past deadline →
   RunFailed("exceeded deadline"). Keep the simple stdout/stderr tails (no
   log-file/markers — that was the sync path's paging feature; async leaves
   stdout_path/bytes empty).
4. **Cedar:** Poll/Cancel for Agent; ExecStarted as system; computer_exec_start/
   poll in the http_call/access_secret module scoping.
5. **build.sh + app.toml:** add computer_exec_start + computer_exec_poll.
6. **Panel scripts (stack):** create + Run a Computer.Exec, poll the row to
   Succeeded/Failed, read the tail — behind PANEL_EXEC=governed (raw `tl sbx exec`
   default until proven).
7. **Combined e2e:** C lifecycle (Copy→Leased→forced timeout→Destroyed) + a
   governed Exec through the async poll incl. a >120s command (proves the poll
   survives invocation boundaries) + Cancel. Real provider calls per C5 (prod
   verify). Local proves the state machines + poll loop with stubs.

## D open question for the loop
- Whether a Poll trigger may return WITHOUT set_result on the not-done branch
  (relying on the Poll transition's timeout re-arm), or must report a benign
  KeepRunning self-loop. Confirm against the host at first e2e; default to
  KeepRunning if a no-result return is rejected.

## Encodes from the getrandom regression (team-lead requested)
- **E1 — a dependency added for one module's need must not poison the shared
  helper crate for targets that don't use it.** wasm-helpers is a shared path dep
  of many modules; adding `getrandom` (a hard `compile_error!` on
  wasm32-unknown-unknown) broke consumers that never draw randomness. Shared
  crates take target-portable dependencies only; a target-specific need uses a
  cfg'd raw import (DCE'd when unused), not a crate that fails to compile on some
  targets. Strongest rung available now = this note + the fix; a CI lint that
  shared wasm crates carry no target-breaking deps would be the durable rung.
- **E2 — several consumer modules are still wasm32-unknown-unknown against the
  standing wasip1 rule.** The getrandom break only EXPOSED this; the modules
  (context_preparer, provider_auth_gate, provider_caller, and others) should be
  wasip1 per the deploy rule. Team-lead is filing this as its own small issue —
  NOT C/D scope.

## D-FINAL — poll-loop semantics settled at kernel source (open question closed)

**Decision:** The async-exec poll loop re-arms via an explicit `reset_on = ["Poll"]`
on the `Running` `state_timeout`, and `computer_exec_poll`'s not-done branch reports
SUCCESS with an EMPTY callback action (no transition). The `KeepRunning` action is
DELETED. The same finding forced `reset_on = ["Heartbeat"]` on the `Leased`
`state_timeout` so a copy's Heartbeat actually renews its lease.

**Came up because:** the D open question ("may a Poll trigger return without a
result on the not-done branch, or must it report a benign KeepRunning?"). The
team-lead asked to settle it empirically and prefer the no-result re-arm if the
kernel accepts a triggered module reporting nothing.

**What the kernel source proves (rev 43f9379, the running build):**
- A triggered WASM module reporting an EMPTY callback IS accepted: the engine
  parses `result.callback_action` as `parsed.get("action").unwrap_or("")`
  (`temper-wasm/src/engine/mod.rs:339`), and the dispatch path runs the callback
  only `if !callback_action.is_empty()`, otherwise returns `Ok(None)` — no error,
  no stall (`temper-server/src/state/dispatch/wasm.rs:485`). So the not-done branch
  should report success with no action. (Reporting NOTHING at all is different:
  an unset result is read as `success:false` → on_failure=RunFailed. So the module
  must call `set_success_result("", …)` explicitly.)
- BUT a self-loop does NOT re-arm a `state_timeout` on its own. Arming
  (`temper-server/src/state/dispatch/state_timeouts.rs:225-244`) computes
  `state_changed = pre_state != post_state`; for a self-loop that is `false`, so
  `is_entry` is false and the timer is re-armed only when
  `is_reset = !state_changed && reset_on.contains(action)`. Without the action in
  `reset_on`, the branch `continue`s and the timer is NOT re-armed.

**Two latent bugs this exposed (both in my own committed D/C specs), now fixed:**
1. The `Running` timeout had no `reset_on`, so the Poll loop would have fired
   exactly ONCE and then stalled in Running forever. `KeepRunning` did not save it —
   it is also a self-loop and equally fails to re-arm. Fix: `reset_on = ["Poll"]`
   on the Running timeout; delete `KeepRunning`; not-done branch reports `("", {})`.
2. The `Leased` timeout had no `reset_on`, so `Heartbeat` (documented as "renews
   the lease") was a silent no-op — the lease would have fired 3600s after the copy
   went live regardless of heartbeats. Fix: `reset_on = ["Heartbeat"]`.

**Options:** (a) keep KeepRunning as the not-done callback — REJECTED: it is pure
machinery that does not even re-arm (self-loop), so it would have masked bug #1
without fixing it; (b) no-result return with no set_result — REJECTED: read as
failure → RunFailed; (c) explicit empty-callback success + reset_on — CHOSEN: the
kernel-blessed "report nothing" signal, with the re-arm carried by the one
mechanism that actually re-arms a self-loop.

**Chose (c):** gained a correct, minimal loop (one fewer action, one fewer Cedar
entry) and a correctly-renewing lease; gave up nothing — the deleted KeepRunning
never worked as imagined.

**Where:** `os-apps/paw-compute/specs/exec.ioa.toml` (Running `reset_on`, KeepRunning
removed), `computer.ioa.toml` (Leased `reset_on`), `policies/compute.cedar`
(KeepRunning removed), `os-apps/paw-compute/wasm/computer_exec_poll/src/lib.rs`
(not-done → `set_success_result("", …)`). Kernel citations above.

## E3 — self-loop state_timeouts need explicit reset_on (encode)
A `[[state_timeout]]` on a state that a self-loop action re-enters is NOT re-armed
by that self-loop unless the action is listed in the timeout's `reset_on`. Any
keep-alive / poll loop built on a self-loop MUST declare `reset_on = ["<action>"]`,
or it fires once and stalls. Two specs in this very effort shipped the bug before a
kernel-source read caught it. Strongest available rung: this note; a spec-lint that
flags a self-loop action targeting a state whose `state_timeout` lacks it in
`reset_on` would be the durable rung (candidate for temper-spec's parser, which
already validates reset_on names).

## Round 1 (panel 2/3) — 8 act-ons, batched

**R1.1 (CRITICAL) — async copy (the copy had D's exec problem).** `computer_copy`
did ONE synchronous `sandbox_copy` blocking up to 240s, inside the ~120s WASM cap —
a real live-copy of arni-big (minutes) would die mid-invocation every time. Fixed
the same way as D's exec: split into `computer_copy_start` (a short-timeout POST
that returns the new sandbox id promptly → `CopyStarted` → new `Copying` state) and
`computer_copy_poll` (a `Copying` state_timeout, `reset_on=["CopyPoll"]`, health-
checks readiness across invocations → `CopyComplete`→Leased; past the deadline or on
a hard failure → `CopyExpired`→Terminating, which tears down the leaked COPY —
machine_id is the copy's by then). `Destroy` now also allows `Copying`. Where:
`specs/computer.ioa.toml`, `wasm/computer_copy_start`, `wasm/computer_copy_poll`,
`policies/compute.cedar`, `app.toml`, `build.sh`.

**R1.2 — CSDL drift.** `model.csdl.xml` was missing the new surface. Added Computer
`SourceMachineId`/`CopyDeadlineAtMs` + actions Copy/ProvisionFromCopy/CopyStarted/
CopyPoll/CopyComplete/CopyFailed/CopyExpired/Heartbeat/TerminateComplete, and Exec
`RunId`/`StartedAtMs`/`DeadlineAtMs` + ExecStarted/Poll/Cancel. (Dispatch reads the
spec, not the CSDL — which is why the e2e worked — but the served $metadata contract
was stale.) Where: `specs/model.csdl.xml`.

**R1.3 — poll read truncates at the source.** `tensorlake_exec_poll` read the FULL
stdout/stderr into WASM memory before the caller truncated. Now fetches only the
last `tail_bytes` of each stream via an HTTP Range request (`read_file_tail`), with a
caller-side tail as a backstop if the provider ignores Range — a multi-GB output can
no longer OOM the module. Where: `wasm-helpers/src/sandbox.rs`.

**R1.4 — no delete before the result commits.** The poll deleted the rc/output files
before the terminal callback committed; a crash in between lost the result. It no
longer deletes — the capture files are ephemeral and die with the sandbox when the
copy is Destroyed. Where: `wasm-helpers/src/sandbox.rs` (`tensorlake_exec_poll`).

**R1.5 — idempotent start (no duplicate orphan on retry).** `sandbox_exec_start`
minted a NEW random run_id per attempt on a non-idempotent POST. Now the caller
passes a DETERMINISTIC run_id derived from the Exec row id (`deterministic_run_id`,
one exec per row), and the launch wrapper is idempotent (rc-file check + `flock`), so
a retried `Run` trigger cannot spawn a second process. Where:
`wasm-helpers/src/sandbox.rs`, `wasm/computer_exec_start`.

**R1.6 — transient vs terminal in the poll.** One Temper-read or provider blip →
`RunFailed` used to kill a live exec. `computer_exec_poll` now treats a resolve/poll
error as "still running" (empty callback, reset_on re-fires) BEFORE the deadline, and
only fails terminally once past it. Where: `wasm/computer_exec_poll`.

**R1.7 — single deadline source.** The run limit lived in two modules (start's
`MAX_RUN_SECS`, poll's `MAX_RUN_MS`) against the host clock. `computer_exec_start` now
stamps `deadline_at_ms` on the Exec row once; the poll only reads and compares it.
Where: `specs/exec.ioa.toml` (field + ExecStarted param), `computer_exec_start`,
`computer_exec_poll`.

**R1.8 — a copy never inherits the source's name.** `Copy`'s `copy_fields` dropped
`name` (an attach/resolution key — a copy named "arni-big" would collide with the
source in attach-by-name and box_resolve); `computer_copy_start` sets a distinct
`copy-<child-id>` name at `CopyStarted`. Where: `specs/computer.ioa.toml`,
`wasm/computer_copy_start`.

**Consider taken — poll backoff.** For a 30-min exec, 10s ticks = ~180 invocations.
`computer_exec_poll` now backs off after the first minute: it still fires every 10s
(and re-arms) but only hits the provider ~1 tick in 3 (~30s), using elapsed time (no
new field). Where: `wasm/computer_exec_poll`.

**Known residual (documented):** `computer_copy_start`'s POST is not perfectly
idempotent (the provider mints the copy id, so a retried start after a created-but-
unreported copy leaks a sandbox). It is caught by the lease timeout / the panel's
stale-copy reaper — the same net as CopyFailed's existing leak note. Not a new class.

## Round 2 (panel 2/3) — 3 act-ons, all in the tail-read edges

The round-1 Range-read fix had three edge holes. Reworked to bound output AT THE
SOURCE instead of at read time, which closes all three cleanly (HttpResponse
exposes only status+body — no headers, no size cap — so a safe Range probe was not
possible: any GET that the server answered with a full body would already have
buffered it into WASM memory).

**R2.1 + R2.2 — no full-body read, ever; and status-checked.** The launch wrapper
now writes `tail -c {tail_bytes}` `.tail` files on the box, and the poll reads ONLY
those bounded files (`read_capture_tail`), so a gigabyte of output is truncated
before it leaves the sandbox — the Range fallback that could accept a full-body 200
is gone. `read_capture_tail` returns a body only on a 2xx status; a 404 (no output)
or a 5xx error page becomes an EMPTY tail, never the error page itself. Where:
`wasm-helpers/src/sandbox.rs` (`tensorlake_exec_start` wrapper, `tensorlake_exec_poll`,
`read_capture_tail`), `computer_exec_start` (passes the tail bound).

**R2.3 — bounded on-box retention.** The wrapper deletes the full stdout/stderr on
SUCCESS ONLY (rc == 0), after the bounded `.tail` files are written, so a durable
computer never accumulates unbounded output; on FAILURE the full files are KEPT for
debugging. The result is never lost (the `.tail` the poll reports is written before
the delete, and rc — which signals completion — is written last). The retention rule
is noted in `exec.ioa.toml` next to the output fields. This honors the round-1
"result before delete" ordering while removing the unbounded-growth path. Where:
`wasm-helpers/src/sandbox.rs`, `specs/exec.ioa.toml`.
