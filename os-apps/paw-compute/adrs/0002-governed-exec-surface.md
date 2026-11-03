# ADR-0002: Governed exec surface — Exec entity + computer_exec WASM

Date: 2026-08-23
Status: Accepted

## Context

With ADR-0001, third-party harnesses can attach a Computer by reading its
row, but the only way to run commands on the sandbox behind it was the
provider CLI (tl) from a laptop, with the provider API key held locally.
Those invocations bypass Temper entirely: no Cedar gate, no audit record,
no way to see from the platform what was executed on the metal.

The platform already has all the exec plumbing: wasm-helpers' sandbox
module dispatches command execution per provider (sandbox_exec →
tensorlake_exec / modal_exec), with credentials resolved from the module
config overlay, and monty_repl/coding sessions use it today. What was
missing is an entity that exposes that plumbing as a governed action.

## Decision

Add an Exec entity to paw-compute (Created → Running → Succeeded | Failed).
One Exec row is one shell command on one Computer's sandbox:

- `Run(computer_id, command)` transitions Created → Running and
  fires the new `computer_exec` WASM integration.
- The module resolves the Computer row via the Temper API loopback,
  requires it to be Ready with a recorded sandbox_url, builds a
  SandboxHandle from its fields (provider, sandbox_url, machine_id), and
  executes the command through wasm_helpers::sandbox::sandbox_exec —
  reusing the existing provider dispatch rather than adding a second exec
  path.
- The module reports back with `RunSucceeded(exit_code, stdout_tail,
  stderr_tail)`; trigger failure dispatches `RunFailed(error)`. Both
  terminal states are invariant-final.

Cedar: agents may create/read/list Execs and dispatch Run; the callbacks
are admin-only (WASM dispatch path); http_call and access_secret are
scoped to `context.module == "computer_exec"`.

## Consequences

- Every command executed on a computer is an entity: Cedar-gated at Run,
  auditable after the fact from the row (command, exit code, output
  tails); WHO ran it is kernel-stamped on the events (non-spoofable), not a
  caller-supplied field.
- Provider credentials (tensorlake_api_key, modal tokens) stay host-side,
  injected through the trigger config overlay at runtime. Harnesses never
  see or hold them; nothing secret lives in the spec or repo.
- stdout/stderr are truncated to an 8 KB tail each before landing on the
  row. Full output retrieval (e.g. writing to a TemperFS file) is a
  follow-up if needed.
- Long-running and interactive commands are out of scope for v1: the
  trigger executes synchronously within the sandbox exec's polling budget,
  and there is no stdin, no streaming, and no cancel. Commands that outlive
  the budget fail with a timeout error on the row.
- Exec rows reference the Computer by id; nothing prevents racing a Sleep —
  the Ready check is best-effort at dispatch time, and a command against a
  computer that sleeps mid-flight fails and records the failure.
