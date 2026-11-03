# WASM build artifacts

The app builders must package the output from the directory they pass to Cargo.
They honor CARGO_TARGET_DIR; without it they explicitly use the module's target
directory. Required output or build failures must exit nonzero.

Run `bash scripts/test-wasm-build-artifacts.sh`. It drives all 13 builders with
default, absolute, relative, missing-output and failed-Cargo cases, and compares
each package byte-for-byte with that module's output. It also checks both paw-fs
package destinations. This fixture substitutes Cargo to isolate packaging from
unrelated module dependencies.

For a real build, plant a distinct stale file under a module's default
`target/wasm32-wasip1/release` path, then run
`CARGO_TARGET_DIR=/tmp/wasm-proof-target bash os-apps/paw-compute/wasm/build.sh`.
Compare each packaged module with the redirected output using `cmp`, and confirm
it differs from the planted file. Also build with CARGO_TARGET_DIR unset.

Load and invoke one real packaged module with the deployed-compatible
temper-wasm engine. Check its actual result and callback; compiling source or
printing imports alone is insufficient. The never-provisioned Computer
termination path is a safe success case with no provider side effect.

For a tooling rollout, verify the installed checkout's commit and rerun the
builder in the actual agent environment. Preserve existing dirty worktrees and
report which checkouts must refresh. Do not call a source test a deployment.
