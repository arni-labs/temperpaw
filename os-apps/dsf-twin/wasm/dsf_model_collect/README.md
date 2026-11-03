# Flow and participant observations

`dsf_model_collect` reads sources for `DsfFlow` and `DsfParticipant` subjects.
Provider resources use their own typed collectors. This module never creates
resource rows or dispatches actions; ModelSync's declared reactions record its
immutable `DsfObservation` results.

The source File contains `subject_type`, `subject_id`, `provider_id`,
`secret_name`, `interval_seconds` (60–86400), and `source`. The subject fields
must match ModelSync's `subject_type` and `resource_id`. Only real `DsfFlows` and
`DsfParticipants` routes are permitted.

Supported sources:

| `source.provider` | Fields | Subject |
| --- | --- | --- |
| `github` | `owner`, `repository`, `git_ref` | DsfFlow |
| `datadog` | `site`, `app_key_secret`, `query`, `window_seconds`, `max_age_seconds` | DsfFlow or DsfParticipant |
| `dsf_operations` | `service`, `environment`, `max_age_seconds` | DsfFlow or DsfParticipant |

GitHub observations retain the exact resolved commit, tree, and commit time.
They omit author details and commit messages. Datadog observations retain bounded
numeric points and the exact query/window. A zero is a measured value; no numeric
points are absence of data, and old points are stale data. Neither condition
asserts an outage. A metric query does not create or impersonate a Datadog monitor.
A participant subject may represent a cohort measured by a Datadog query. Request
counts measure requests, not unique people; synthetic probes included by the
query remain part of that measurement. The cohort definition must state these
limits, and observations retain the exact query and time window.

Operational snapshots use the authenticated DSF endpoint with
`participant_limit=200&job_limit=20`. The returned participant cursor becomes
ModelSync's `source_cursor`, which the next collection passes as
`participant_cursor`. The final page clears the cursor so the next cycle starts
at the beginning. Each page records its start and next cursor in a separate
immutable observation. A last page after a nonempty start cursor is not described
as a complete inventory, and this module never replaces or deletes participant
rows from a partial page. Agents reconcile those explicit observations with the
application model.

The parser checks snapshot version, service/environment, echoed page limits,
source timestamp, revision format, and cursor shape. Unknown product fields are
omitted. Provider errors record access status without response bodies or secrets.
Host failures that cannot produce a safely bound callback remain visible through
the integration failure and existing ModelSync retry behavior.
