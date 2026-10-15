# Spec
Two jobs. `checks` (fast, all events): fmt, clippy, cargo check, bash -n over worker scripts, Swatinem/rust-cache. `full` (non-PR events: main/release push, nightly cron 07:00, dispatch): executed smokes, all os-app WASM builds, full cargo tests incl. the five OTS-contract module workspaces, dashboard build.
