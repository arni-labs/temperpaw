# 2026-05-18 Reply Delivery Timeout RCA

## Incident Evidence

Datadog showed the user-visible Discord failures were not a normal provider HTTP
status response. They were host-side HTTP deadline failures followed by a
Channel reply invariant warning:

- 2026-05-18T21:33:24Z and 2026-05-18T21:34:35Z:
  `WASM host call exceeded outer deadline; returning error to guest`
  with `custom.host_fn=host_http_call` and `custom.timeout_secs=60`.
- 2026-05-18T21:33:25Z and 2026-05-18T21:34:36Z:
  `integration returned without state transition -- invariant violation`
  on `Channel.SendReply`, entity
  `en-019d9109-f010-7ff1-bcb0-72700a94ef23`, state `Connected`.
- 2026-05-18T21:46:11Z:
  `context_compactor: calling openai_codex at https://chatgpt.com/backend-api/codex/responses`.
- 2026-05-18T21:47:11Z:
  another `host_http_call` deadline.
- 2026-05-18T21:47:13Z:
  `Channel.SendReply` invariant warning, followed by `delivering discord reply`
  for thread `1018228973869727785`.

The same 21:32-21:49Z window had high runtime pressure:

- 676 liveness coverage warnings.
- 281 unmet intent warnings.
- 28 skipped oversized REPL state persists.
- 2 `Event budget exhausted (10000 max)` warnings.
- 2 bounded orphaned-session recovery warnings.
- 10 burst `review-quality` CurationJob submissions from 21:27:40Z through
  21:28:00Z, plus one earlier at 21:21:11Z.

Cron evidence: the created CronJobs only ran `cron_compute_next` in activate
mode and computed next run times. No observed `CronJob.Trigger` log submitted
the review jobs in the inspected window. The actual review work came from direct
`CurationJob.Submit` activity, not from the cron.

## Change

`agent_reply` no longer dispatches `Channel.SendReply` with
`await_integration=true`. Agent sessions enqueue the reply action and leave
transport delivery to the Channel entity's `send_reply` integration.

`Channel.send_reply` now has an explicit 30 second timeout budget and keeps the
existing `ReplyFailed` failure transition.

ADR recorded: `os-apps/paw-channels/adrs/001-nonblocking-reply-delivery.md`.

## Verification

Red tests first:

- `agent_reply_dispatches_channel_reply_without_waiting_for_delivery` failed on
  the old blocking URL.
- `channel_send_reply_trigger_is_bounded_and_reports_delivery_failure` failed
  because `send_reply` did not have a 30 second timeout.

Green:

- `cargo test -p temperpaw --test session_turn_architecture` passed.
- `cargo build --target wasm32-unknown-unknown --release` passed in
  `os-apps/paw-agent/wasm/agent_reply`; the local `agent_reply.wasm` artifact
  was rebuilt.
- `cargo test -p temperpaw` passed.

## Residual Risk

This change removes one failure amplifier: completed sessions no longer block on
reply webhook delivery. It does not by itself solve provider/compactor admission
control, event-budget exhaustion, or invalid Katagami review jobs from Draft
state. Those require Temper-native follow-up work: admission entities for
provider/compactor capacity and curation state guards that only submit quality
review from a valid review state.

## Follow-Up: 2026-05-18T22:01Z Recurrence

Datadog showed the same production symptom at 2026-05-18T22:01:10Z while
production was still running version `2f7f718b0a62817896884bc29345ae421d37cf3d`.
The sequence was:

- 2026-05-18T22:00:09Z:
  `context_compactor: calling openai_codex at https://chatgpt.com/backend-api/codex/responses`.
- 2026-05-18T22:01:09Z:
  `WASM host call exceeded outer deadline; returning error to guest`
  with `custom.host_fn=host_http_call` and `custom.timeout_secs=60`.
- 2026-05-18T22:01:10Z:
  `Channel.SendReply` invariant warning, followed by `delivering discord reply`
  with `custom.content_len=70`.

Additional fix:

- `context_compactor` now uses local fallback compaction immediately for
  `openai_codex` sessions instead of making a background Codex compaction call.
- Non-auth compaction transport/provider failures fall back locally instead of
  failing the Session.
- `agent_reply` sanitizes known provider transport failures so raw backend URLs
  are not sent to Discord.
- Session admission now caps `ProviderAuthReady` at 3 and
  `CompactionAuthReady` at 1 so curation bursts are gated at the provider
  boundary, not only at Session creation.
- ADR recorded:
  `os-apps/paw-agent/adrs/010-compaction-fallback-and-provider-error-sanitization.md`.

Additional verification:

- Red tests failed first for missing compaction fallback and reply sanitization.
- `cargo test` passed in `os-apps/paw-agent/wasm/context_compactor`.
- `cargo test` passed in `os-apps/paw-agent/wasm/agent_reply`.
- `cargo test -p temperpaw --test session_turn_architecture` passed.
- Release WASM artifacts were rebuilt for `context_compactor` and `agent_reply`.
