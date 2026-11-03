# TemperPaw Datadog Observability Guide

Status: live verified for production on 2026-05-13.

This guide describes the observability that is now present for the actual
TemperPaw system, how to navigate it as a human, and how agents should query it.
It intentionally distinguishes proven telemetry from account/tooling limitations.

## Live Snapshot

Current production runtime:

- TemperPaw version: `86bd073dc89efc6e559cbdf9787ce9e0b92228fe`
- Temper revision used by the live production proof:
  `64824d640a915272e21a307029030439a41fdde5`
- Temper mainline revision pinned by the PR-ready source after the Temper
  squash merge:
  `01288d7bbbbf30f68d5353d3e8982aad5be49ee0`
- Temper revision pinned by the PR-ready source for WASM guest observability:
  `9f094a48f35f79a8a2d1195082f54b99f377e8b3`
- Note: the live proof SHA above is the pre-squash Temper branch commit. The
  mainline SHA is the merged equivalent used by current source so future
  builds depend on `main`, not an orphaned PR branch.
- Runtime build label: `sha-86bd073`
- Railway deployment: `598c9ca9-f026-40c0-9b95-f086d82fe846`
- GHCR source image:
  `ghcr.io/nerdsane/temperpaw:sha-86bd073`
- GHCR source digest:
  `sha256:9859786cdbdbc72c76417e94531497b16c04df4af0b4a115a0def7a58d604e3c`
- Railway deployment image digest:
  `sha256:0a532ec01863d6a72bd8f24f4d5dcdf47eea0de304b258b86c0ec0aeffbc85a1`
- `/readyz`: HTTP 200 during proof
- `/paw/version`: `{"version":"sha-86bd073","sha":"86bd073dc89efc6e559cbdf9787ce9e0b92228fe"}`

Canonical Datadog identity:

- Service: `service:temperpaw`
- Environment: `env:prod`
- Team tag: `team:temperpaw`
- LLM application: `ml_app:temperpaw`
- Database instance: `database_instance:temperpaw-postgres`

External Railway and object-storage resource names that still carry the previous
product identity are intentionally documented only in
`docs/temperpaw-legacy-identity-allowlist.md`. Treat that file as the source of
truth for what is temporarily allowed and why.

## What Is Observable

TemperPaw is now observable through these Datadog surfaces:

- APM traces for HTTP routes, OData actions, long-lived `Session.workflow`
  execution, WASM integration dispatch, host-function boundaries, Postgres
  spans, and artifact publishing.
- Logs with shared fields for session, entity, action, trace, span, WASM module,
  workflow step, provider phase, file/blob identifiers, and deployment version.
- Metrics for traffic, WASM host HTTP behavior, Cedar evaluations, blob/FS
  behavior, session budgets, DBM activity, log-derived errors/warnings, and
  profiling uploads.
- Datadog Profiling upload health plus an authenticated on-demand CPU profile
  path for the Rust runtime.
- Postgres DBM samples with SQLCommenter propagation and APM calling-service
  metadata.
- LLM Observability spans for agent, workflow, and LLM calls with token counts,
  provider/model tags, latency, status, and trace correlation.
- Dashboard, monitors, log pipeline, log metrics, source-controlled facets, and
  source-controlled Sensitive Data Scanner rules.

The intended operating model is: start from the session id or trace id, inspect
the APM tree for chronology and bottlenecks, pivot into logs for details, use
LLMObs for provider/model/token behavior, use DBM for query samples, and use
profiling when a CPU path is suspected.

## Railway Datadog Product Coverage

Railway production has one supported observability profile:

- `datadog-enhanced-railway` sends TemperPaw runtime OTLP to the
  `datadog-runtime-agent` Railway service at
  `http://datadog-runtime-agent.railway.internal:4318`, sends Datadog trace
  client traffic to
  `http://datadog-runtime-agent.railway.internal:8126`, and enables direct
  LLMObs export with `DD_LLMOBS_API_ENABLED=true`. It also includes
  `dd_llmobs_enabled=false` in `OTEL_RESOURCE_ATTRIBUTES` so Datadog keeps OTLP
  spans in APM instead of auto-converting them into duplicate LLMObs rows.

Deployments without a Datadog key run with `OTEL_ENABLED=false` and export no
telemetry.

For an already-deployed Railway instance, call
`POST /paw/infra/railway/datadog-runtime-agent/ensure`. The endpoint uses the
stored Railway deployment token internally, creates or reuses the
`datadog-runtime-agent` service from `datadog/agent:7`, sets the Runtime Agent
and TemperPaw Datadog-enhanced variables, persists
`railway_datadog_runtime_agent_service_id`, and redeploys the Agent and app
without returning the Railway token or Datadog API key.

For live product-boundary proof, call
`GET /paw/infra/railway/datadog-capability-check` on the Railway app. It returns
the same USM and continuous-profiler capability fields as
`./scripts/datadog_railway_capability_check.sh` without requiring shell access
to the container. To run the temporary continuous profiler canary, call
`POST /paw/infra/railway/datadog-continuous-profiler-canary` with
`{"enabled":true}`; this sets `TEMPER_DDPROF_ENABLED=true` and
`DD_PROFILING_ENABLED=true` on the app service, then redeploys it. After proof,
call the same endpoint with `{"enabled":false}` to restore
`TEMPER_DDPROF_ENABLED=false` and `DD_PROFILING_ENABLED=false`.

Product status is green or proven blocked, not guessed:

| Product | Railway status |
| --- | --- |
| APM | Supported by `datadog-runtime-agent` APM intake. |
| Logs correlation | Supported by decimal Datadog trace/span ids plus OTel ids. |
| Error Tracking | Supported by normalized exception and Datadog error fields. |
| LLM Observability | Supported by direct LLMObs export. |
| On-demand Profiling | Supported by `/_admin/profile/cpu` capture and upload. |
| Continuous Profiling | Canary-gated; record `blocked-on-Railway-perf-permissions` if Railway denies OS perf APIs. |
| Universal Service Monitoring | Capability-gated; record `blocked-on-Railway-system-probe` if Railway cannot provide system-probe mounts/capabilities. |

The proof contract is: on-demand profiling remains supported even if continuous `ddprof` is blocked on
Railway. USM is not considered misconfigured when Railway cannot expose the host
kernel access Datadog system-probe requires.

To prove backend Error Tracking, call authenticated `POST
/paw/infra/datadog/error-tracking-synthetic` with a JSON body such as
`{"proof_id":"dd-error-proof-<timestamp>"}`. The endpoint emits
`DatadogSyntheticBackendError` as an errored OpenTelemetry span named
`datadog.error_tracking.synthetic` and as a backend error log. The payload carries
`error.type`, `error.kind`, `error.message`, `error.stack`,
`exception.type`, `exception.message`, and `exception.stacktrace`. Verify it in
the Error Tracking Explorer by searching `service:temperpaw env:prod
DatadogSyntheticBackendError` or by the proof id, then link the issue in the
proof report.

Run `GET /paw/infra/railway/datadog-capability-check` or
`./scripts/datadog_railway_capability_check.sh` inside the Railway TemperPaw
container or the temporary continuous-profiler canary before marking USM or
continuous profiling green. The expected blocked statuses are
`blocked-on-Railway-system-probe` for missing system-probe host access and
`blocked-on-Railway-perf-permissions` for continuous profiler OS permission
failures.

## Core Query Vocabulary

Use these fields first. They are deliberately shared between humans, agents,
logs, traces, metrics, and proof documents.

| Concept | Datadog field |
| --- | --- |
| Service | `service:temperpaw` |
| Environment | `env:prod` |
| Version | `version:<git sha>` or `service.version:<git sha>` |
| Session | `@session_id:<session id>` |
| Entity | `@entity_type:<type>` and `@entity_id:<id>` |
| Action | `@action_name:<action>` |
| State transition | `@from_status:<state>` and `@to_status:<state>` |
| Trace join | `trace_id:<decimal trace id>` |
| OTel join | `@otel.trace_id:<32-char hex trace id>` |
| Span join | `span_id:<span id>` and `@otel.span_id:<span id>` |
| LLM provider/model | `@gen_ai.provider.name:<provider>`, `@gen_ai.request.model:<model>` |
| LLM app | `ml_app:temperpaw` |
| WASM module | `@wasm_module:<module>` |
| WASM workflow step | `@workflow_step:<step>` |
| Guest progress | `@progress.kind:<kind>` |
| Tool call | `@tool.name:<tool>` and `@tool.call_id:<call id>` |
| Transport | `@observability_event:temperpaw.transport` |
| Webhook | `@observability_event:temperpaw.webhook` |
| Approval | `@observability_event:temperpaw.approval` |
| File/blob | `@workspace_id:<id>`, `@file_id:<id>`, `@content_hash:<sha256>` |
| Artifact publication | `@artifact_id:<id>`, `@public_storage_key:<key>` |
| Postgres peer | `@peer.service:temperpaw-postgres` |
| Errors | `status:error`, `@error.kind:*`, `@error.message:*` |

## Agent Session Trace

The primary session proof is:

- Session: `ss-019e2239-fe6f-7810-b717-d842442bfce1`
- Prompt: `Reply exactly: TemperPaw 86bd073 final Datadog observability verified.`
- Result: `TemperPaw 86bd073 final Datadog observability verified.`
- Provider/model: `openai_codex`, `gpt-5.5`
- Runtime state flow:
  `Created -> Provisioning -> PreparingContext -> EnsuringProviderAuth -> CallingProvider -> ApplyingProviderResponse -> Steering -> Completed`

APM proof:

- APM trace id: `3582885463604920100`
- OTel trace id observed in DBM propagation:
  `5cee74dbd4bd9d9631b8f758a114ff24`
- Root resource: `Session.workflow`
- Root span id: `12367668208079438116`
- Root duration: `21.523s`
- Span count observed by raw APM search: `524`
- Datadog trace link:
  `https://app.datadoghq.com/apm/trace/3582885463604920100?graphType=flamegraph&shouldShowLegend=true&spanID=12367668208079438116&timeHint=1778690686000.0000&trace=358288546360492010012367668208079438116&traceQuery=`

Useful shape in APM:

```text
Session.workflow
  Session.Configure / dispatch phases
  Session.WorkspaceReady.integrations
    wasm:workspace_provisioner
      wasm.host.* boundary spans
      Postgres spans
  Session.ContextReady.integrations
  Session.ProviderAuthReady
    provider_auth_gate
    provider_caller
    LLM/LLMObs correlation
  Session.CheckSteering.integrations
    steering_checker
  Session.FinalizeResult
  Session.MarkTrajectoryEmitted
```

The trace is intentionally long and expandable rather than made of many
detached fragments. The root stays open across the background workflow, and
internal details appear as action, dispatch, WASM, host-function, Postgres, and
provider spans under the same session chronology.

Useful APM searches:

```text
trace_id:3582885463604920100
service:temperpaw @entity_id:ss-019e2239-fe6f-7810-b717-d842442bfce1
service:temperpaw resource_name:Session.workflow
service:temperpaw @wasm_module:workspace_provisioner
service:temperpaw type:sql @db.system:postgresql @peer.service:temperpaw-postgres
```

## WASM Host Boundary Visibility

Datadog APM does not automatically see inside a WASM guest. Temper makes WASM
observable from the host/runtime side and correlates guest logs and progress
events with the active session trace.

High-signal host spans to expect:

- `wasm.host.http_call`, `wasm.host.http_call_binary`, and
  `wasm.host.http_stream`
- `wasm.host.connect_call`
- `wasm.host.get_secret`
- `wasm.host.evaluate_spec`
- `wasm.host.cache_contains`, `wasm.host.cache_to_stream`,
  `wasm.host.cache_from_stream`, `wasm.host.read_field`, and
  `wasm.host.hash_stream`

Important fields:

- `tenant`
- `entity_type`
- `entity_id`
- `trigger_action`
- `action_name`
- `session_id`
- `wasm_module`
- `workflow_step`
- `workflow_root_entity_type`
- `workflow_root_entity_id`
- `workflow_run_id`
- `trace_id`
- `span_id`

Guest progress is searchable as `wasm_guest.progress` structured logs/events
with `progress.kind`, `workflow_step`, `tool.name`, `success`, `trace_id`,
`span_id`, `dd.trace_id`, and `dd.span_id`.

Do not expect a span for every guest statement. These are host-boundary spans,
not inside-WASM APM spans. The intended shape is:

```text
agent session -> workflow/action -> WASM invocation -> meaningful host boundary spans
```

Smaller details belong in guest progress events, logs, metrics, and entity
state. ADR-0086 in Temper records why explicit guest-created child spans are
deferred until there is a tested host API such as `host_start_span`,
`host_end_span`, or `host_add_span_event`.

## Logs

Logs should answer "what happened around this session or trace?" without
guesswork.

Useful searches:

```text
service:temperpaw @session_id:ss-019e2239-fe6f-7810-b717-d842442bfce1
service:temperpaw trace_id:3582885463604920100
service:temperpaw @otel.trace_id:5cee74dbd4bd9d9631b8f758a114ff24
service:temperpaw @wasm_module:* @workflow_step:*
service:temperpaw @gen_ai.request.model:gpt-5.5
service:temperpaw status:error
```

Final session log proof:

- Query: `service:temperpaw "ss-019e2239-fe6f-7810-b717-d842442bfce1"`
- Returned logs under version `86bd073dc89efc6e559cbdf9787ce9e0b92228fe`
- Logs included provider/steering completion and OTS trajectory emission:
  `trj-ss-019e2239-fe6f-7810-b717-d842442bfce1`
- Trace id visible on correlated logs: `3582885463604920100`

Required facets are source-controlled in `dd-pipelines/facets.json`. The log
pipeline `TemperPaw / Temper Logs (ADR-0054)` is applied in Datadog with id
`Wyq_6z_fTviM9uVH9MUIrQ`. Log-derived metrics include
`temperpaw.logs.errors`, `temperpaw.logs.warns`, and
`temperpaw.logs.wasm.default_timeout_fallback`.

Sensitive-data scanner source rules live in
`dd-pipelines/sensitive-data-scanner.json`. They cover common Datadog, LLM,
source-control, chat, email, and cloud-token patterns. Applying those rules
requires Datadog scanner group context in the UI for this account.

## LLM Observability

Open LLM Observability with `ml_app:temperpaw`.

TemperPaw emits LLMObs through the direct LLMObs span payload path. The Datadog
Runtime Agent still receives runtime OTLP traces for APM, but TemperPaw intentionally
does not forward generic OTLP spans to the LLMObs intake; otherwise internal
workflow, SQL, and WASM phase spans show up in LLMObs as `No content` rows.
Use LLMObs for agent/workflow/LLM/tool content and APM for the lower-level
runtime trace.

Provider LLM spans must include structured `gen_ai.input.messages`,
`gen_ai.output.messages`, and `gen_ai.system_instructions` attributes. The
provider success path also emits a `gen_ai.client.inference.operation.details`
event with `gen_ai.output.messages` so Datadog can populate the LLMObs content
view instead of rendering the provider response as `No content`.

Final LLMObs proof:

- Trace id: `123527112440865216744564245077429649188`
- APM/log session: `ss-019e2239-fe6f-7810-b717-d842442bfce1`
- Span count: `3`
- Tree depth: `3`
- Error count: `0`

Observed tree:

```text
agent temperpaw.agent.session
  workflow Session.ProviderAuthReady
    llm wasm:provider_caller
```

LLM span:

- Span id: `5439141549405964232`
- Provider: `openai`
- Model: `gpt-5.5`
- Duration: `2.333s`
- Input tokens: `1114`
- Output tokens: `19`
- Total tokens: `1133`
- Status: OK

Useful LLMObs searches:

```text
ml_app:temperpaw span_kind:agent
ml_app:temperpaw @tags:"session_id:ss-019e2239-fe6f-7810-b717-d842442bfce1"
ml_app:temperpaw @status:error
```

Recommended workflow:

1. Search for recent `span_kind:agent` roots or filter by `session_id`.
2. Open the LLMObs trace tree to verify agent -> workflow -> LLM nesting.
3. Inspect the LLM span details for provider, model, tokens, duration, and
   status.
4. Pivot to APM with the session id when you need workflow, WASM, Postgres, or
   file/blob context.

Known helper limitation: Datadog's `get_llmobs_agent_loop` helper returned an
empty timeline for the direct Session trace even though the LLMObs trace tree is
correct. Use `get_llmobs_trace`, APM, logs, and Temper entity history as the
authoritative chronology for this direct Session path.

## Postgres DBM

Postgres DBM is live for `database_instance:temperpaw-postgres`.

Final correlated sample:

- Timestamp: `2026-05-13T16:44:53.801Z`
- Service: `temperpaw`
- Database instance: `temperpaw-postgres`
- Database: `railway`
- Table: `entity_field_index`
- Query signature: `94651ed8bdbcaeb0`
- Statement class:
  `INSERT INTO entity_field_index ... ON CONFLICT ... DO UPDATE`
- APM trace id: `3582885463604920100`
- APM span id: `357018841880160397`
- Trace mode: `full`
- Calling service: `temperpaw`
- Calling resource: `Session.workflow`
- Calling version: `86bd073dc89efc6e559cbdf9787ce9e0b92228fe`
- SQLCommenter traceparent:
  `00-5cee74dbd4bd9d9631b8f758a114ff24-04f462bec063688d-01`

Useful DBM and APM pivots:

```text
source:postgres service:temperpaw database_instance:temperpaw-postgres
service:temperpaw type:sql @db.system:postgresql @peer.service:temperpaw-postgres
@query_signature:94651ed8bdbcaeb0
trace_id:3582885463604920100
```

DBM samples are sampled. A specific short HTTP route may have APM SQL spans
without a matching DBM sample, but session proof shows full APM/DBM correlation
for the active deployment.

## Profiling

Profiling is available through the authenticated Temper pprof endpoint and is
uploaded to Datadog through the Railway Datadog Agent service.

Final profiling proof:

- Request: authenticated `/_admin/profile/cpu?seconds=5&frequency=100`
- HTTP status: `200`
- Content type: `application/vnd.google.protobuf`
- Downloaded size: `83` bytes
- Profile hash:
  `sha256:a702af1125e50891c7ab96e35073489fd38a25e6eb6191641711a55747db8e49`
- Capture start log: `2026-05-13T16:51:45.768Z`
- Capture complete log: `2026-05-13T16:51:50.859Z`
- Upload log: `2026-05-13T16:51:51.062Z`
- Current-version metric:
  `sum:datadog.profiling.rust.profiles_uploaded{service:temperpaw,env:prod,version:86bd073dc89efc6e559cbdf9787ce9e0b92228fe}.as_count()` returned one point at `2026-05-13T16:52:00Z`
- Matching upload-error series returned no data in the same check

Useful searches:

```text
service:temperpaw ("ADR-0055" OR "profile uploaded") version:86bd073dc89efc6e559cbdf9787ce9e0b92228fe
sum:datadog.profiling.rust.profiles_uploaded{service:temperpaw,env:prod}.as_count()
sum:datadog.profiling.rust.upload_errors{service:temperpaw,env:prod}.as_count()
```

On Railway, profiling uploads are on-demand. The paging monitor is therefore
`[Temper] Profiler Upload Failures`; absence of a recent manual profile is not
itself a production fault.

## Runtime RSS / OOM Guard

Process RSS is the host-level signal for projection, query, replay, and
prepared-context materialization regressions that can kill the Railway runtime
before request-level counters explain the failure.

The May 22, 2026 OOM follow-up established this baseline:

- Failing versions reached roughly `7.6-8.0 GB` RSS before restarts.
- The fixed `sha-72ad9b9` rollout settled around `1.3-1.4 GB` RSS.
- The live hot `SessionEntries` SQL-page path returned p50 `36.416ms`, p95
  `43.599ms`, max `44.671ms`, with `candidate_count=1`,
  `materialized_count=1`, and `pushdown_sparse_skip_reason=sql_page`.
- DBM sampled the corresponding entity-catalog SQL page query at roughly
  `4.701ms` CPU on `temperpaw-postgres`.

Paging guard:

- Monitor: `[TemperPaw] Process RSS OOM Guard`
- Query:
  `max(last_10m):max:process_resident_memory_bytes{service:temperpaw,env:prod} by {host,version} > 6500000000`
- Warning: `4.5 GB`
- Critical: `6.5 GB`

When this monitor fires, first compare host and version on the Process Memory
dashboard, then inspect `odata.entity_set_read` spans for large
`candidate_count` or `materialized_count`, replay parity probes, and prepared
context inline artifact volume. The expected healthy shape for the fixed sparse
page path is `pushdown_sparse_skip_reason=sql_page` with page-sized candidate
and materialized counts.

## TemperFS Blob & Document Services

TemperFS and published artifacts are observable as file/blob events, APM spans,
Postgres spans, object-store logs, and public-read proof.

Useful TemperFS and blob pivots:

```text
service:temperpaw @workspace_id:<workspace id>
service:temperpaw @file_id:<file id>
service:temperpaw @fs.operation:create_file
service:temperpaw @content_hash:<sha256>
service:temperpaw @observability_event:temperpaw.blob
service:temperpaw @observability_event:temperpaw.fs
```

Dashboard metrics to start with:

- `temper_blob_transport_wait_duration_ms`
- `temper_blob_local_fast_path_requests_total`
- `temper_blob_native_transport_duration_ms`
- `temper_blob_native_transport_requests_total`
- `temper_blob_native_transport_request_bytes`
- `temper_blob_native_transport_response_bytes`

Use `temper_blob_io_wait_duration_ms` for semaphore queue wait and
`temper_blob_native_transport_duration_ms` for the actual native BlobStore
transport boundary. For File `$value` residual latency, pivot native transport
by `operation`, `backend`, `outcome`, and `status_code_class` before deciding
whether the next fix belongs in R2/S3 client behavior, local filesystem I/O,
direct upload, or actor/event commit work.

Final publish proof:

- Route: `POST /api/files/publish-artifact`
- Deployment owner: `598c9ca9-f026-40c0-9b95-f086d82fe846`
- Source file: `bootstrap-soul-file-paw`
- Source status: `Ready`
- Source content hash:
  `sha256:a7b843737b4e8d4eaab95a060898b7abbaad53b4b618dcbe2c18b14e5a7eeaa9`
- Response: HTTP 200 in `631.753ms`
- Artifact id: `part-33863cb7a1bc3906a4819ac56ddcfcc5`
- Public URL:
  `https://temperpaw-assets.katagami.ai/codex-live-proof/CodexProof/598c9ca9-f026-40c0-9b95-f086d82fe846/codex-live-publish-86bd073-rich-telemetry-v2-a7b843737b4e8d4eaab95a060898b7abbaad53b4b618dcbe2c18b14e5a7eeaa9.md`
- Public read: HTTP 200, content length `18568`
- Downloaded object hash matched the source hash

APM publish proof:

- APM trace id: `d1f2b8c57fcf4858fd5bea0aeb5bbdf6`
- Root span: `http.server.request POST /api/files/publish-artifact`
- Route span: `POST /api/files/publish-artifact`
- State span: `state.publish_file_artifact`
- Child/sibling spans:
  `state.read_file_stream_indexed`, `state.put_public_blob`,
  `postgres.upsert_published_artifact`, and SQL spans for
  `published_artifacts`
- Trace link:
  `https://app.datadoghq.com/apm/trace/d1f2b8c57fcf4858fd5bea0aeb5bbdf6?graphType=flamegraph&shouldShowLegend=true&spanID=6934239732066560871&timeHint=1778690633928.0625&trace=d1f2b8c57fcf4858fd5bea0aeb5bbdf66934239732066560871&traceQuery=`

Required publish logs were observed for:

- `public blob PUT succeeded`
- `published artifact metadata persisted`
- `publish artifact request completed`

Those logs include artifact id, storage key, public URL, byte length, MIME type,
source file id, content hash, owner ref, metadata backend, service version, and
trace/span fields.

Useful searches:

```text
service:temperpaw "publish artifact request completed"
service:temperpaw @artifact_id:part-33863cb7a1bc3906a4819ac56ddcfcc5
trace_id:d1f2b8c57fcf4858fd5bea0aeb5bbdf6
service:temperpaw resource_name:state.put_public_blob
service:temperpaw resource_name:postgres.upsert_published_artifact
```

Operational rule: a 200 from `publish-artifact` proves the API returned. It is
not complete proof by itself. Also verify `state.put_public_blob` HTTP 200,
metadata persistence, Postgres `published_artifacts` spans, and a public URL
read whose content hash matches the source.

## Channel Transports

Transport logs expose inbound/outbound Slack and Discord operations without
logging message bodies:

```text
service:temperpaw @observability_event:temperpaw.transport
service:temperpaw @transport.name:slack
service:temperpaw @transport.name:discord
service:temperpaw @transport.operation:receive_message
service:temperpaw @transport.outcome:error
```

## Webhook Triggers

Webhook triggers expose the trigger boundary:

```text
service:temperpaw @observability_event:temperpaw.webhook
service:temperpaw @webhook.route_key:<route key>
service:temperpaw @webhook.event_id:<event id>
service:temperpaw @webhook.outcome:error
```

## Governance Approvals

Approval waits expose both the blocked session and the human notification path:

```text
service:temperpaw @observability_event:temperpaw.approval
service:temperpaw @decision_id:<decision id>
service:temperpaw @session_id:<session id>
service:temperpaw @approval.operation:notify_human
service:temperpaw @approval.outcome:error
```

## Sandbox & Modal Bridge

Sandbox and bridge work is visible through structured logs and WASM host HTTP
metrics:

```text
service:temperpaw @observability_event:temperpaw.sandbox
service:temperpaw @sandbox_provider:modal
service:temperpaw @sandbox_id:<sandbox id>
service:temperpaw @sandbox.operation:bash
service:temperpaw @modal_bridge.operation:create
```

Start with `temper_wasm_host_http_requests_total` and
`temper_wasm_host_http_duration_ms` when bridge calls are failing or slow. If a
bridge call fails before a sandbox id exists, inspect runtime configuration for
`modal_bridge_url` and then check host HTTP metrics for the same window.

## Metrics, Dashboard, And Monitors

Source-controlled assets:

- Dashboard: `dd-dashboards/temperpaw-overview.json`
- Monitors: `dd-monitors/temperpaw-monitors.json`
- Log metrics: `dd-log-metrics/temper-log-metrics.json`
- Log pipeline: `dd-pipelines/temper-temperpaw.json`
- Facets: `dd-pipelines/facets.json`
- Sensitive Data Scanner rules: `dd-pipelines/sensitive-data-scanner.json`

Datadog dashboard:

- Dashboard id: `mn4-k3k-i66`
- Title: `TemperPaw - Platform Overview`
- Purpose: single pane of glass for session health, LLM behavior, Postgres DBM,
  profiling, logs, trace health, sandbox/transport/webhook/approval/TemperFS
  surfaces, and bottleneck signals.

Live metric proof from the current proof window included:

- `temper_wasm_host_http_requests_total{service:temperpaw}` activity
- `temper_cedar_evaluations_total{service:temperpaw}` activity
- `process_resident_memory_bytes{service:temperpaw,env:prod}` activity by host
  and version
- `postgresql.queries.count{service:temperpaw,database_instance:temperpaw-postgres}` activity
- fresh DBM explain plans for `service:temperpaw` / `temperpaw-postgres`
- `datadog.profiling.rust.profiles_uploaded{service:temperpaw,env:prod,version:86bd073dc89efc6e559cbdf9787ce9e0b92228fe}` activity

Monitor proof:

- `search_datadog_monitors(query:"tag:team:temperpaw")` returned the
  TemperPaw monitor set.
- `search_datadog_monitors(query:"tag:team:temperpaw status:alert")` returned
  no active alerting monitors during final verification.

Important monitors include:

- `[TemperPaw] Error Rate Spike`
- `[TemperPaw] Agent Session Trace Correlation Missing`
- `[TemperPaw] LLM Error Rate Spike`
- `[TemperPaw] Postgres DBM Query Latency Regression`
- `[TemperPaw] Postgres DBM Query Metrics Missing`
- `[TemperPaw] TemperFS Metadata Operation Errors`
- `[TemperPaw] Sandbox Host HTTP Error Spike`
- `[Temper] Required WASM Load Failures`
- `[Temper] Profiler Upload Failures`
- `[TemperPaw] Process RSS OOM Guard`

The DBM integration-alive monitor gates on query-count telemetry rather than
sparse `datadog.dbm.activity_rows` samples:

```text
sum(last_30m):default_zero(sum:postgresql.queries.count{service:temperpaw,database_instance:temperpaw-postgres}.as_count()) < 1
```

Use DBM activity rows, DBM samples, and explain plans diagnostically for
correlation and query-shape work; they can be sparse even when the Postgres DBM
integration, query count metrics, and explain plans are live.

## Agent Query Surface

Agents should use the Temper Datadog query surface for credential-gated reads.
Supported query kinds are:

- `monitor_status`
- `recent_events`
- `metrics_query`
- `logs_query`
- `trace_query`
- `llmobs_query`
- `dbm_query`
- `profiling_query`

Example agent queries:

```python
temper.datadog_query({
    "query_kind": "logs_query",
    "query": "service:temperpaw @session_id:ss-019e2239-fe6f-7810-b717-d842442bfce1",
    "from": "now-30m",
    "to": "now",
})

temper.datadog_query({
    "query_kind": "trace_query",
    "query": "trace_id:3582885463604920100",
    "from": "now-2h",
    "to": "now",
})

temper.datadog_query({
    "query_kind": "llmobs_query",
    "ml_app": "temperpaw",
    "span_kind": "agent",
    "from": "now-2h",
    "to": "now",
    "include_attachments": False,
})

temper.datadog_query({
    "query_kind": "dbm_query",
    "query": "source:postgres service:temperpaw database_instance:temperpaw-postgres",
    "from_ts": 1778688000,
    "to_ts": 1778691600,
})

temper.datadog_query({
    "query_kind": "profiling_query",
    "query": "sum:datadog.profiling.rust.profiles_uploaded{service:temperpaw,env:prod}.as_count()",
    "from_ts": 1778688000,
    "to_ts": 1778691600,
})
```

## Human Runbooks

To debug a failed or slow agent session:

1. Start in APM with the session id or `Session.workflow`.
2. Confirm the root duration, child count, and state/action order.
3. Expand slow `Session.*.integrations`, `wasm:*`, `wasm.host.*`, Postgres, and
   provider spans.
4. Pivot to logs with `@session_id` and the decimal `trace_id`.
5. Open LLMObs with `ml_app:temperpaw` and the session tag for token/provider
   behavior.
6. Check DBM if SQL spans or entity hydration are slow.
7. Capture a CPU profile only when the trace/logs suggest runtime CPU cost.

To debug artifact publication:

1. Search APM for `POST /api/files/publish-artifact`.
2. Expand `state.publish_file_artifact`.
3. Verify read, blob PUT, metadata upsert, and SQL children.
4. Search logs for artifact id and `published artifact metadata persisted`.
5. Curl the returned public URL and compare content length/hash.

To debug observability itself:

1. Confirm `service:temperpaw`, `env:prod`, `team:temperpaw`, and version tags.
2. Check monitor status for `tag:team:temperpaw`.
3. Query profile upload logs and metrics.
4. Query DBM samples for `database_instance:temperpaw-postgres`.
5. Query LLMObs for `ml_app:temperpaw span_kind:agent`.
6. Use the identity allowlist doc for external resource names that cannot be
   renamed without a cutover.

## Known Limitations

These are documented limitations, not hidden proof gaps:

- Some external Railway/object-storage resource names remain on the explicit
  allowlist until a planned cutover. Runtime product identity remains
  `service:temperpaw`.
- The Datadog `get_llmobs_agent_loop` helper returned an empty direct-session
  timeline. The LLMObs tree, APM trace, logs, and Temper entity history are the
  authoritative chronology.
- Log facet registration and Sensitive Data Scanner application require
  Datadog UI/account context in this account. Source definitions and tests live
  in the repo.
- The plain `openai` provider path failed because a secret template remained
  unresolved in runtime config. The final proof used the working
  `openai_codex` provider. Treat the plain provider path as a config gap, not
  an observability gap.
