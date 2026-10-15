# DSF twin

This app is a live model of Deep Sci-fi: its running infrastructure, application
flows and real participant activity. All changes to DSF go through this model. The
factory is the surrounding lifecycle: Intent, Effort and Ask in paw-patrol, computers
in paw-compute, and Foundry. DSF's product code stays in its existing repository.
Intent, Effort, Ask, File and Computer remain the existing Temper contracts.

The installation and live proof are tracked in [ARN-467](https://linear.app/arni-build/issue/ARN-467).
Source and passing local tests do not establish which revision is installed.

## Resources and actions

| Resource type | Actual identity | Owned operations |
|---|---|---|
| DsfRailwayServiceInstance | Project, service and environment IDs | Deploy, ApplyConfiguration, Rollback |
| DsfVercelProject | Account/team and project IDs | Deploy, ApplyConfiguration, Rollback, SetAlias |
| DsfSupabaseProject | Supabase project reference | ApplyConfiguration |
| DsfCloudflareR2Bucket | Cloudflare account ID and bucket name | ApplyConfiguration |
| DsfDatadogMonitor | Organization, site and actual monitor ID | ApplyConfiguration |
| DsfMediaPipeline | DSF application environment, API resource and bucket resource | RetrySelected |

Every resource also owns RefreshObservations. The API and Datadog collector on
Railway are separate service instances. Vercel previews and aliases belong to
their actual project; an alias does not create another project. Datadog metric
queries describe evidence on a Flow, while monitor rows represent actual monitors.

Use full provider identity for the stable entity ID. Labels such as
`DSF / production / API / Railway` are display text. A renamed label retains its
identity; a recreated provider resource receives a new identity. All introduced
types and sets have the Dsf prefix, but installation must still check ownership.

The [generated module contract](specs/module-contracts.json) supplies exact entity
sets, registration parameters, human actions, callbacks and verification flags.
The CSDL and IOA declarations are generated together. Do not add a provider switch
or an operation entity between a resource and its own actions.

## A resource operation

An agent registers the target with a configuration File ID and the SHA-256 of its
exact bytes. ResourceConfig version 3 contains the concrete provider target,
verification expectations and required Ask IDs. Credential fields name tenant
secrets; the File never contains credential values. The target File is reread and
hashed at every stage.

The [configuration contract](wasm/dsf_resource_common/README.md) binds verification
to a registered application and its provider-owned origin. An observation-only
discovery can be explicitly unbound. Agents maintain configuration bindings and
dependencies through ReviseModel with the current model sequence and provenance;
provider identity remains fixed and earlier operation verification is invalidated.

For example, ApplyConfiguration on the production Railway API can request
`{"numReplicas":2}`. That resource's action records the Effort, operation key,
expected current operation sequence, requested configuration and proof reference.
A deployment also names the full source revision. Configuration changes do not
invent a deployment revision.

The state machine sequences validation, one provider write, provider observation
and application verification. Each stage is a separate, statically bound WASM.
The same binary can serve multiple instances of its one resource type. No WASM
invokes another Temper transition.

Validation checks the actual linked Effort and proof, permitted resource action,
required Asks and an exact resource_change entry in the proof artifact. That entry
binds resource ID, type, action, operation key, accepted sequence, revision and the
SHA-256 of the exact requested configuration bytes. The proof commit identifies
the tested source.

Callbacks carry the original key and accepted sequence. An uncertain provider
response enters reconciliation; it cannot authorize an unobserved second write.
Railway and Vercel deployments are never resent after an ambiguous creation.
Idempotent configuration updates can retry only after a provider read confirms
absence. Exhausted reads remain visible and can be resumed explicitly without
resetting the provider write count.

Verification requires the correct provider result, the affected application flow
and the matching successful Datadog request. The shared operation_verified flag
and the matching per-action flag become true only after VerificationSucceeded.
Acknowledging failure may return the resource to Active but does not turn old
evidence into a verified operation.

The Effort retains the release plan, review, proof and completion record. Resource
delivery records the exact operations it expects; agents invoke those resource
actions after the merge gates pass. The Effort's read-only completion check must
confirm those exact operations. Existing TemperPaw image delivery remains a
separate supported Effort path.

## Observations

A resource's collector reads only its registered provider target. It records an
immutable DsfObservation before applying the sequence-checked current projection.
A late projection cannot overwrite newer facts, but its evidence record remains.
Measured, absent, inaccessible and stale outcomes stay distinct. Failed reads
retain previous measured facts and mark availability explicitly.

Railway's observed revision comes from the active deployment, not a queued latest
build. No Data is a measured Datadog monitor state, not evidence of health. Missing
media API access does not mean the application is absent. A production media
repair additionally proves the linked Railway custom domain and R2 media domain
before a paid request.

DsfModelSync owns bounded GitHub/code, Flow and Participant observation. Idle and
Ready sources check their recorded due time every minute; a future due time skips
all provider reads and creates no observation. Explicit Refresh collects immediately.
Pause stops scheduling; failed collections retain their bounded retry policy. Each active
resource separately refreshes every five minutes through its own state timeout.
Collection failure rearms that timer; operations suspend it until Active, and
retirement stops it. Participant
pages preserve cursor and page-scoped coverage; one page is not the full inventory.
Sampled traces, metrics, jobs and participants retain distinct units. Observations
never adopt intended configuration or authorize repairs. Agents investigate drift
and record decisions through the existing Effort and Ask flow.

## Isolated experiments

One DsfExperiment represents one variant, with exact source, computer, database,
bucket, namespace and permitted external calls. Existing governed Exec records
run bounded commands on that Computer. The runner verifies actual isolated
bindings, records receipts for restart recovery, and refuses production data and
media. Compare separate variant records; selection validates the existing Ask and
returns to ordinary delivery. Cleanup targets only the experiment's owned files,
database and bucket. Production staging names do not prove isolation.

## Build and verify

```sh
python3 os-apps/dsf-twin/specs/generate.py --check
python3 os-apps/dsf-twin/wasm/generate_modules.py --check
bash os-apps/dsf-twin/wasm/build.sh
cargo test -p temperpaw --test dsf_factory_contract
cargo test -p temperpaw --test dsf_resource_wasm
python3 -m unittest discover -s os-apps/dsf-twin -p test_names.py
```

The build derives the module manifest from the executable triggers and packages
binaries where Temper's loader expects them. Missing modules and generated drift
fail the build. The WASM proof invokes the actual packaged binaries. Provider
fixtures establish code behavior; live provider, Datadog and browser proof are
required separately before deployment is complete.

Before installation, fetch fresh metadata from the target tenant and run:

```sh
python3 os-apps/dsf-twin/check_names.py --tenant default --live-metadata /path/to/target-metadata.xml
```

For upgrades, also supply --installed-record and --installed-model exported from
the running installation and its pinned Genesis bundle. The record contains
tenant, app_name, app_ref and model_sha256. A candidate branch or matching namespace
cannot establish ownership. Existing DsfDeploy and DsfDeploys are outside this app.
