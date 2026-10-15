# DSF model synchronization contracts

Drive this surface when changing DsfModelSync scheduling, collector behavior, or callback correlation in `os-apps/dsf-twin`.

From the repository root, run:

```bash
cargo test --manifest-path os-apps/dsf-twin/wasm/dsf_model_collect/Cargo.toml
cargo test -p temperpaw --test dsf_factory_contract model_sync
cargo test -p temperpaw --test dsf_resource_wasm packaged_model_collector_defers_only_scheduled_future_reads
```

Record the exact source revision, packaged collector SHA256, commands, and output. `CARGO_TARGET_DIR` can reuse an existing test cache. Confirm that all selected tests execute and pass.

The collector cases verify bounded provider requests, evidence handling, freshness, and explicit versus scheduled reads. Contract tests drive the production actor timer through deferral, rearm, pause, and stale callback refusal. The packaged-module case invokes the actual WASM with simulated host responses and checks that only a scheduled future read is deferred.

These checks prove local runtime contracts. They do not prove that the canonical app is installed or that a live model refresh occurs. For a delivery claim, additionally install the authorized exact app revision through the supported native tools, read its ModelSync, observe a real collection and a later scheduled collection, and read the resulting observation and provenance. Cover installation and governed dispatch through the existing Genesis and OData feature entries. Do not substitute simulated callbacks for those observations.
