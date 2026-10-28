# ARN-66 Foresight Port And Topology Proof

Date: 2026-06-18

## Scope

TemperPaw worktree: `/Users/seshendranalla/.codex/worktrees/0be7/temperpaw`
Branch: `codex/arn-66-foresight-port`
Remote: `origin=https://github.com/nerdsane/temperpaw.git`

This proof covers the ARN-66 port/readiness lane for the Deep Sci-Fi/Foresight engine:

- confirm canonical deployment topology;
- check whether searched-corridor fixes landed only in openpaw or also in canonical Foresight;
- port the prompt/file-write recipe delta to GitHub main;
- avoid duplicating ARN-65's broader self-heal lane.

This is not a fresh productive end-to-end run proof. It prepares the path for one.

## Deployment Topology Evidence

Railway project `openpaw-seshendranalla` contains both services:

- `openpaw` service id `4a8dedaa-8a2e-4cdd-945b-e06c781bb3f0`
- `foresight` service id `ed6c91b5-d235-4286-964a-41acdab47c49`

Canonical Foresight evidence:

- Railway service config source image: `ghcr.io/nerdsane/temperpaw:sha-e1c3968`
- latest deployment: `5110497a-...`, SUCCESS at `2026-06-18 13:21:41 UTC`
- database host: `aws-1-us-west-1.pooler.supabase.com`
- `TEMPER_POSTGRES_MAX_CONNECTIONS=40`
- `DD_SERVICE=foresight`
- `DD_DBM_DATABASE_SERVICE=foresight-supabase`
- `LLM_PROVIDER=openai_codex`
- `LLM_MODEL=gpt-5.5`
- public readiness: `https://foresight-production-72d1.up.railway.app/readyz` returned ready
- Genesis bootstrap ref includes `temperpaw/paw-foresight@659e40c663024af1acf1ed6d2a39d872d3dbdf14`

Openpaw comparison evidence:

- Railway service config source image: `ghcr.io/nerdsane/temperpaw:sha-5c3c05f`
- latest deployment: `b1b96383-...`, SUCCESS at `2026-06-18 15:23:06 UTC`
- database host: `postgres.railway.internal`
- `DD_SERVICE=temperpaw`
- `DD_DBM_DATABASE_SERVICE=temperpaw-postgres`
- `LLM_PROVIDER=openai_codex`
- `LLM_MODEL=gpt-5.5`
- Genesis bootstrap ref includes `temperpaw/paw-foresight@01ac826b9604ef1828eee146724a44953375ebfb`

Conclusion: canonical deployment for this run track is `foresight` on Supabase, not `openpaw` on Railway Postgres. Both checked deployments are still configured for Codex/OpenAI provider `openai_codex` with model `gpt-5.5`; no provider switch is part of this work.

## Commit And Genesis Evidence

Local Git evidence:

- `e3f2fbaf fix(foresight): give session agents an explicit file-write recipe (no API-guessing)` is not an ancestor of `origin/main`.
- `22795542 fix(foresight): self-heal the endpoint writer phase (Endpoint.Sampled)` is not an ancestor of `origin/main`.
- both are present on the searched-corridor branch, not GitHub main.

Genesis evidence:

- canonical `paw-foresight@659e40c663024af1acf1ed6d2a39d872d3dbdf14` has 90 files and includes `wasm/sample_endpoints/src/lib.rs`, `specs/endpoint.ioa.toml`, and the other corridor WASM modules.
- canonical `paw-foresight@659e40...` contains Endpoint self-heal markers in `specs/endpoint.ioa.toml`, including `Sampled`, `ResumeWriter`, `state_timeout`, `allow_indefinite_states`, and `UnderRepair`.
- canonical `paw-foresight@659e40...` also contains earlier file-write guidance in WASM prompt source, but still contains the bare `<file-id-from-temper.write>` placeholder in all six checked WASM prompt modules.
- openpaw `paw-foresight@01ac826b9604ef1828eee146724a44953375ebfb` has 29 files and does not include the checked `paw-foresight` WASM/spec paths, so it is not the canonical app bundle for this lane.

Conclusion: the self-heal class from `22795542` is already visible in the canonical deployed Genesis ref, while GitHub main is missing it. ARN-65 is actively owning the broader self-heal source convergence. This ARN-66 branch therefore ports only the prompt/file-write delta, with a tighter tested recipe than the deployed `659e40...` bundle currently exposes.

## Red Test

Added failing contract tests first in these modules:

- `sample_endpoints`: `writer_prompt_gives_explicit_file_write_recipe`
- `seed_world`: `surveyor_prompt_gives_explicit_file_write_recipe`
- `spawn_adversaries`: `adversary_prompt_gives_explicit_file_write_recipe`
- `spawn_repairers`: `repairer_prompt_gives_explicit_file_write_recipe`
- `render_artifacts`: `author_prompt_gives_explicit_file_write_recipe`
- `animate_dwellers`: `dweller_prompt_gives_explicit_file_write_recipe`

Initial targeted runs failed because GitHub main did not show literal `temper.write("/...md", ...)` calls in the executable prompts and left the ambiguous file-id placeholder.

## Green Change

Executable prompts now tell agents:

- the workspace already exists;
- `temper.write` is the only file creation path;
- not to create Files, Directories, or Workspaces manually;
- the exact write path to call for each phase;
- that `temper.write` returns `file_id`, `path`, and `workspace_id`;
- to pass `result["file_id"]` into the relevant completion action.

Updated agent manuals for surveyor, endpoint writer, adversary, repairer, and dweller to match. The endpoint-writer manual was also aligned from stale `SubmitForRepair` language to the current `BundleWritten` diversity-gate contract.

## Automated Verification

Commands completed successfully after implementation:

```sh
cargo test --manifest-path os-apps/paw-foresight/wasm/sample_endpoints/Cargo.toml
cargo test --manifest-path os-apps/paw-foresight/wasm/seed_world/Cargo.toml
cargo test --manifest-path os-apps/paw-foresight/wasm/spawn_adversaries/Cargo.toml
cargo test --manifest-path os-apps/paw-foresight/wasm/spawn_repairers/Cargo.toml
cargo test --manifest-path os-apps/paw-foresight/wasm/render_artifacts/Cargo.toml
cargo test --manifest-path os-apps/paw-foresight/wasm/animate_dwellers/Cargo.toml
git diff --check
```

Results:

- `sample_endpoints`: 11 passed
- `seed_world`: 7 passed
- `spawn_adversaries`: 8 passed
- `spawn_repairers`: 13 passed
- `render_artifacts`: 7 passed
- `animate_dwellers`: 7 passed
- whitespace check passed

## Deep Sci-Fi UI Status

Read-only inspection found the run-progress UI on local Deep Sci-Fi branch `codex/dsf-2`, not on the deployed Railway service:

- repo: `/Users/seshendranalla/Development/deep-sci-fi-worktrees/dsf-2`
- branch: `codex/dsf-2`, ahead of `origin/codex/dsf-2` by 2 commits
- commits: `dfe5a904 Add live run progress UI with stall detection`, `efcc166f Add e2e coverage for live run progress panel`
- changed files include `platform/components/world/RunProgressLive.tsx`, `platform/app/api/world/[id]/run-status/route.ts`, `platform/lib/run-status.ts`, `platform/lib/temper-server.ts`, and e2e/unit coverage.

The UI path uses `TEMPER_API_URL`, `TEMPER_API_KEY`, and `TEMPER_TENANT` through the server-side helper; no openpaw URL hardcode was found in the run-progress path. The deployed DSF Railway backend is sourced from `arni-labs/deep-sci-fi`, root `platform/backend`, with latest successful deploy `2026-04-04 12:57:10 UTC` at commit `deacc27a...`; its variables do not include the Temper proxy variables needed by the run-progress API. A second DSF repo PR/deploy is required before the run-progress UI can serve as canonical Foresight run proof.

## ADR Judgment

No new ADR was added. This change does not alter entity specs, policies, storage, triggers, deployment behavior, or orchestration architecture; it ports and tests prompt/operator guidance for existing actions and tools.

## Fresh Run Readiness Checklist

- Merge/publish this prompt-file-write port into the canonical `paw-foresight` Genesis app.
- Let ARN-65 finish and publish the broader source convergence for Endpoint.Sampled, UnderRepair, World.Active, and Seeding self-heal, or explicitly pin to the already deployed canonical `659e40...` behavior if that remains the accepted production baseline.
- Configure and deploy the DSF run-progress UI against canonical Foresight with `TEMPER_API_URL=https://foresight-production-72d1.up.railway.app`, the correct tenant, and a server-side API key.
- Start one fresh productive run against `foresight`/Supabase.
- Capture OData proof for the new world: World state, Endpoint state progression, Sessions activity, file ids written by `temper.write`, and final artifacts/stories.
- Confirm in Datadog under `DD_SERVICE=foresight` and `DD_DBM_DATABASE_SERVICE=foresight-supabase` that the run uses Supabase and does not fall back to openpaw Railway Postgres.
