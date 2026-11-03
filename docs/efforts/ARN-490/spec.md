# WASM artifact packaging contract

Each existing builder chooses one output directory before invoking Cargo, passes it explicitly with --target-dir, and verifies/copies only the required artifact from that directory after a successful build. CARGO_TARGET_DIR, including relative paths, is interpreted from the module directory as before. Without it, the builder explicitly uses that module's target directory. An unrelated default-path file cannot satisfy a redirected build. A missing required artifact or failed build produces a nonzero script exit and no success message for that module.

Preserve module lists, WASM target triples, special binary selection, compute's WASI/wbindgen verification and package destinations. Summaries report packaged paths. The existing helper owns directory selection/build/output validation; callers own package destinations and app-specific checks.

Acceptance: run every affected caller with distinguishable stale/default and fresh/redirected artifacts; check normal, relative and spaced directory paths; force absent output and failed Cargo. Build a real module, compare bytes to the new output, execute it through the deployed-compatible kernel. Verify corrected scripts in the actual Foundry/Tensorlake checkout workflow and state precisely which existing sessions require refresh.
