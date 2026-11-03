# Plan: land the paw-compute app
Spec: docs/efforts/ARN-443/spec.md

## What we are addressing
paw-compute's code runs in prod via Genesis while PR #462 is unmerged — main and
the shelf have diverged. Land the source on main, reconciled to Genesis, building
against the merged ARN-401 fix. Repo-side only; no Genesis publish.

## Approach
Rebase #462 on post-#468 main, verify the branch matches Genesis HEAD (it does,
file-by-file), fix the two things that block landing (app.toml provider string,
SDK rev skew), and take it through the full loop.

## Steps
1. Rebase claude/paw-compute-access on main (has #468). [clean — no sandbox.rs
   conflict; #462 only touches os-apps/paw-compute/]
2. Diff branch vs Genesis HEAD (73646d4) — confirm source identical. [done]
3. Fix app.toml "via Fly.io" → "via Tensorlake".
4. Align computer_exec temper-wasm-sdk rev to main's 43f9379.
5. Build + test: cargo test (computer_exec 17/17), wasm32-wasip1 blob clean
   (WASI imports, zero wbindgen).
6. Effort chain + PR (ARN-443 in the title), gates, panel, merge.

## Files / surfaces touched
- os-apps/paw-compute/app.toml
- os-apps/paw-compute/wasm/computer_exec/Cargo.toml
- (the rest of os-apps/paw-compute/* lands as-is from the branch)

## Expected end state
paw-compute source on main == Genesis HEAD source; builds clean against main;
app.toml provider correct. Genesis untouched. C/D follow as their own efforts.
