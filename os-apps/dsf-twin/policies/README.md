# DSF twin permissions

Run `python3 policies/generate.py` after changing the IOA actions or packaged modules. `--check` rejects an outdated policy. The generator takes resource commands from `module-contracts.json` and classifies every remaining declared action as a runtime callback. The retained model types have explicit command lists.

Registered `dsf-factory` agents and registered `human` members may create resources, request supported operations and read the model. They cannot create immutable observations or submit collection and verification results. Those actions have explicit forbids for other principals, so an unrelated permit cannot grant them.

The kernel's `wasm-runtime`, `timeout-scheduler` and declared `dsf-factory-runtime` service principals may perform the internal actions. Only a declared DsfExperiment reaction under `dsf-factory-runtime` can create an Exec or invoke its Run action. An ordinary agent cannot use that permission.

Host HTTP access belongs to the named compiled modules. Each module validates its provider URL and request before using the host. Secret access also requires a named module and its exact allowed secret IDs: the shared Temper record credentials, its provider's `dsf_*` credentials, and the DSF/Datadog credentials needed by verification. Source configurations cannot expose unrelated tenant secrets.

`crates/temperpaw/tests/dsf_factory_policy.rs` evaluates this generated policy with the real Cedar engine, registered identities, service identities and forged headers.
