# ARN-462 — TemperPaw image break (getrandom)

Temper #455 merged. TemperPaw #500 and #501 merged. Production MCP is
still `sha-63db71e7` / temper `43f9379`. A new kernel reaches this MCP
only by riding a TemperPaw GHCR image. Docker step 3 never produced one.

## What failed

`os-apps/paw-patrol/wasm/build.sh` on `wasm32-unknown-unknown`. The first
new #500 crate in that loop is `chain_file_ready`; it compiles. The next
crate, `chain_github_ready`, pulls `rsa` 0.9 → `getrandom` 0.2.17, which
`compile_error!`s unless feature `js` or `custom`. `js` pulls
wasm-bindgen and is forbidden. `release_run_lifecycle` already has the
`custom` stub (ARN-460). The door crate copied the JWT mint and not the
stub. ARN-460 recorded that skip as a known gap.

PR `checks` do not run os-app `build.sh` (ARN-429). That is why #500 and
#501 looked green and Docker still died.

## Expected end state

- `chain_github_ready` compiles for `wasm32-unknown-unknown` without the
  `js` feature.
- A PR-path check fails if any os-app crate depends on `rsa` and does not
  enable getrandom `custom`.
- `TemperDeploy.Request` waits for the GHCR tag; it does not write
  Railway until the manifest is 200.
- A `ContractProbe` row (`mcp-455-lists`) records the 455 empty-equality
  latency after Healthy and again every 6h.
- Live `/paw/version` still reports `sha-63db71e7` until that deploy.
