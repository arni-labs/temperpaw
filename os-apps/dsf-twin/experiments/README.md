# DSF experiments on the governed computer

`DsfExperiment` binds an immutable manifest File, its exact SHA256, a full DSF
Git revision, an experiment branch, computer, database and media namespace.
The four WASMs prepare commands or read their receipts. Native reactions start
the existing `Exec` entity as `dsf-factory-runtime`; they never call a provider
from a new host orchestrator.

Each phase has a deterministic Exec ID. Resume reads that ID first. A created
Exec can start; a previously started Exec only returns to polling. Every result
contains the original experiment sequence. The poll reads a stored deadline
using the injected clock and records an uncertain outcome when it expires.

The runner is installed on `arni-big` under
`/home/tl-user/work/arn467-experiments/tools/<SHA256>.pyz`. Build it with
`python3 build_runner.py OUTPUT.pyz` and put the returned digest in the immutable
manifest field `runner_sha256`. The command verifies those exact
archive bytes before importing code. It requires the existing computer's
noninteractive sudo, Linux network namespaces, PostgreSQL 16 with pgvector,
MinIO, and a Python venv containing the pinned DSF requirements. The bare DSF
repository belongs at `../repository.git`; unpublished source can arrive as a
private Git bundle without changing its commit.

The manifest accepts no arbitrary command, endpoint or external call. Version 1
runs the two approved CORS variants and the full proposal/review/world/story
HTTP flow. It permits only `arni-big`, full SHA1 revisions, a
`codex/arn467-<experiment-id>` branch, `dsf_<experiment_id>` database,
`dsf-<experiment-id>` bucket and `experiments/<experiment-id>/` media prefix.
The production Supabase project ID is provider identity, not a PostgreSQL
database name or OID. Validation records the new local cluster identifier and
database OID separately. The application checks disable model/provider calls;
the independent S3 check writes and reads the isolated bucket ownership marker.
They do not test generated media output.

Every command runs inside a fresh network namespace with only loopback. It
starts its own database and media server and stops them at phase exit. Only
owned data directories persist. The application receives a new explicit
environment without provider or model credentials, and fresh registered test
users. A per-experiment file lock serializes phases; fsynced receipts survive
process death before a response. A restart reconciles only processes with the
exact owned command and working directory, using Linux pidfds to prevent PID
reuse from targeting another process. An incomplete product run resets only
its isolated schema; a completed run returns the original receipt.

Cleanup verifies ownership, removes the exact worktree, branch, database and
media directories, and retains receipts and local logs. It cannot adopt an
existing unmarked directory or follow a data-directory symlink. Selection
requires an Answered Ask on the experiment's Effort whose `chose` field is the
experiment ID, plus a delivery Effort linked to an Accepted Intent. Selection
does not deploy or bypass ordinary delivery proof and review.

Run `pytest test_runner.py`, Ruff and Mypy (`--platform linux`), the
`dsf-experiment-common` Rust tests, and the canonical
`crates/temperpaw/tests/dsf_experiment_runtime.rs` actor tests. The `command`
Rust example prints the exact command used by the WASM for real governed Exec
verification. Actual proof records stay outside the repository. Runner and
local actor tests do not prove the new model is installed in production.
