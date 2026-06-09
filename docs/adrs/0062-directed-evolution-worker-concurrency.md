# ADR 0062: Directed Evolution worker concurrency

## Status

Accepted

## Context

Directed Evolution queues one WorkItem per simulated user journey, observer pass, or evaluator pass. A single worker process previously claimed queued Directed Evolution WorkItems serially during boot recovery, so a batch of simulated users appeared to be "running" but only advanced one journey at a time.

That behavior made the human-facing Evolution UI misleading and reduced the usefulness of runtime telemetry. Synthetic usage should produce overlapping app interactions when the operator requests multiple simulated users.

## Decision

The boot WorkItem claim path processes queued Directed Evolution WorkItems with bounded parallelism using the worker's `max_concurrent_runs` budget. Each WorkItem still has its own lifecycle and result record; the worker only changes how many queued items it can actively claim and execute at once.

The telemetry evaluator prompt now treats Datadog app-usage logs and traces as the primary evidence for telemetry-gated stages. It scopes evidence by generic Temper observation metadata such as `de.work_item_id`, `de.variant_id`, and `de.simulated_user_id`, rather than depending on hardcoded producer-specific top-level fields.

## Consequences

Operators can launch multiple simulated user journeys and expect progress to advance concurrently when worker capacity allows.

Observers and telemetry evaluators must verify real runtime app-usage telemetry before producing or accepting telemetry-backed directions. If Datadog has no scoped app-usage logs or traces, the stage should report a telemetry gap rather than inventing a direction from thin trajectory records.
