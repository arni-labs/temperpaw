# Feature: governed compute (paw-compute — Computer attach + Exec)

The `paw-compute` app lets a third-party harness attach a `Computer` (read the
governed row) and run a shell command on its sandbox through a governed `Exec`
entity, instead of talking to the sandbox provider directly. Every `Exec.Run` is
Cedar-gated; the command, exit code, and output tails persist on the row as the
audit trail. The `computer_exec` WASM module carries out the exec via
`wasm_helpers::sandbox` and reports back RunSucceeded / RunFailed.

## What to verify
- `Computer` create/read/list is permitted for authenticated tenant principals
  (attach access — ADR-0001).
- `Exec` (Created → Running → Succeeded|Failed) runs the command on the named
  Computer's sandbox and records exit code + output; callbacks are admin-only and
  http_call/access_secret are scoped to `context.module == "computer_exec"`
  (ADR-0002).
- The `computer_exec` module builds for the host and links `wasm_helpers`.

## How to drive it (rerun)
Repo-side (build + module tests):

```
cd os-apps/paw-compute/wasm/computer_exec
cargo test                                   # module unit tests
cargo build --target wasm32-wasip1 --release # links wasm-helpers; blob for the host
# blob must import WASI and carry zero wbindgen strings:
wasm-tools print target/wasm32-wasip1/release/computer_exec.wasm | grep -c wasi_snapshot_preview1   # > 0
wasm-tools print target/wasm32-wasip1/release/computer_exec.wasm | grep -c wbindgen                 # == 0
```

Pass = module tests green, blob builds, WASI imports present, zero wbindgen.

## Notes
- The live governed-Exec walk (create Exec → Run on a real Computer → Succeeded
  with a real exit code, plus the failure path) runs through the Genesis-published
  app; publishing to Genesis is a separate gated step. It was proven live on the
  `dsf` box (dd-computer) and runs in production today; the repo-side build/test
  here verifies the source that produces it.
