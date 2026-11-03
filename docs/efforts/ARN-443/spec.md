# Spec: land the paw-compute app
Status: accepted. Intent: docs/efforts/ARN-443/intent.md

## Requirements
- The `paw-compute` source on `main` matches (or supersedes) Genesis-installed
  `paw-compute@370cc794`.
- The app builds against current `main` (post-#468 wasm-helpers).
- `app.toml` describes the real provider (Tensorlake, not Fly.io).
- No Genesis publish / prod install in this effort.

## Design
`paw-compute` (unchanged shape, per ADR-0001 / ADR-0002 in the app):
- **ADR-0001 attach access:** Cedar permits create/read/list on `Computer` for
  authenticated tenant principals (the shipped policy had none, so the registry
  was unreachable).
- **ADR-0002 governed Exec:** `Exec` entity (Created → Running → Succeeded|Failed)
  whose `Run` fires the `computer_exec` WASM: resolve the Computer row, require
  Ready + sandbox_url, exec via `wasm_helpers::sandbox`, report back
  RunSucceeded / RunFailed. Cedar scopes callbacks admin-only and
  http_call/access_secret to `context.module == "computer_exec"`.

## Reconciliation with Genesis (the core of this effort)
Genesis `temperpaw/paw-compute` HEAD is `73646d4` ("rebuild computer_exec against
the wasm-helpers ARN-401 fix"), two commits past the installed pin `370cc794`. A
file-by-file diff of Genesis HEAD against the branch's `os-apps/paw-compute/`:
every text file is IDENTICAL (app.toml, specs, cedar, computer_exec/src, ADRs).
The branch source already matches Genesis HEAD. Genesis's only newer content is
the compiled `computer_exec.wasm` blob — which temperpaw gitignores by design
(built at publish time from the wasm-helpers ARN-401 fix, now on main). So there
is nothing to pull from Genesis into the branch.

## Changes made to land on main
- `app.toml` description: "via Fly.io" → "via Tensorlake".
- `computer_exec` `temper-wasm-sdk` rev: `a747f7d` → `43f9379` to match
  wasm-helpers and all on-main modules (the skew broke the build against the
  merged ARN-401 fix — two different `Context` types via the shared path dep).

## Policy / invariants
- Cedar unchanged from the shipped app (attach permits + Exec scoping).
- No Genesis mutation; the installed blob and the Genesis registry are untouched.

## Deferred / out of scope
- Publishing paw-compute to Genesis (separate gated step).
- C (copies as children) and D (panel execs through Exec rows) — later efforts.
