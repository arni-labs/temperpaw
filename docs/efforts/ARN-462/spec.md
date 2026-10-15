# ARN-462 — Image-break spec

## Contract

An os-app crate that depends on `rsa` and is built for
`wasm32-unknown-unknown` must compile on that triple. It does so by
declaring, on that target only:

```toml
getrandom = { version = "0.2", default-features = false, features = ["custom"] }
```

and registering a backend that returns `getrandom::Error::UNSUPPORTED`.
JWT sign does not draw randomness. The backend exists so the crate
compiles.

## TemperDeploy tag-wait

`Request` records `image_tag` / `expected_sha` and enters
`WaitingForImage`. The swap writes Railway `IMAGE_TAG` only after GHCR
returns 200 for `ghcr.io/v2/nerdsane/temperpaw/manifests/{image_tag}`.
A missing tag is `ImagePending`, not a swap. `max_checks` bounds the
wait.

## ContractProbe (455 latency)

A LatencyDiag-class row. `RunScan` takes no parameters. Path, filter,
and `max_ms` are set at create. Default `mcp-455-lists`:
`GET /tdata/DesignLanguages?$filter=id eq '__arn462_missing__'`,
`max_ms = 800`. `TemperDeploy.CheckHealthy` fires `RunScan`. Ready and
Failed re-run every 6 hours.

## Forbidden

- getrandom feature `js` (wasm-bindgen; host instantiation fails).
- Putting `getrandom` on `wasm-helpers` (ARN-443: every consumer then
  fails on unknown-unknown).
- Firing `TemperDeploy.Request` on the old machine against a missing tag.
  After this machine is live, `Request` itself waits; still do not invent
  a tag GHCR will never have.

## Out of scope

Migrating patrol modules from `wasm32-unknown-unknown` to `wasm32-wasip1`
is ARN-447. This spec does not do that.

## Proof

1. `scripts/check-wasm-rsa-getrandom.sh` exits 0.
2. `cargo check --manifest-path os-apps/paw-patrol/wasm/chain_github_ready/Cargo.toml --target wasm32-unknown-unknown` succeeds.
3. The same check for `release_run_lifecycle` still succeeds.
4. Host tests for `chain_github_ready` still pass.
