# DSF resource operation contracts

Drive this surface when changing resource operation validation, callback correlation, proof binding, or verification in `os-apps/dsf-twin`.

From the repository root, run:

```bash
cargo test -p temperpaw --test dsf_resource_wasm
```

Record the exact source revision, packaged module hashes, command, and output. Confirm that every selected test executes and passes. This suite loads and invokes packaged modules through the Temper WASM engine. Its simulated host checks exact proof binding, refusal of unbound discovery, staging isolation from production domains and traces, scheduled collector deferral, and the declared correlated failure callback for each resource action stage.

The suite proves behavior with simulated provider responses. It does not prove a provider deployment, a live telemetry match, or installation of the tested modules. Builder packaging changes also require the existing `wasm-artifacts` drive.

For a delivery claim, dispatch the authorized operation through the real resource, read its resulting state, verify the provider's exact deployed revision, drive the application flow, and attach matching environment and revision telemetry. Installation and governed dispatch also use the existing Genesis and OData feature entries. A local DSF backend smoke run is supplementary application evidence: identify its separate repository revision, isolated database, and test-mode settings. It does not prove TemperPaw startup or live provider delivery.
