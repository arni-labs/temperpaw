# ADR-009: Railway Postgres Collocation for Foresight Proof Runs

## Status

Accepted

## Date

2026-06-18

## Context

The dedicated Railway `foresight` service is used for Deep Sci-Fi corridor
engine proof runs. On 2026-06-18 it was deployed in Railway project
`openpaw-seshendranalla`, production environment, in region
`us-east4-eqdc4a`, but its `DATABASE_URL` still pointed at the Supabase pooler
host `aws-1-us-west-1.pooler.supabase.com`.

That topology keeps every Temper platform/event/query-store operation on a
cross-provider pooler path. Current Foresight performance work needs to remove
network, region, and pooler latency before widening inference concurrency. The
database move does not replace the Lane 2 work to reduce database work
itself: snapshots, projections, OTS, and `Session` / `SessionEntry` scans still
need to be reduced.

Railway production already contained an empty, ready Postgres service
`Postgres-v79y` in the same environment and region as `foresight`, with a
private host on the Railway internal network and a ready 50 GB volume.

## Decision

For current Foresight development and proof runs, `foresight` should use a
dedicated collocated Railway Postgres database instead of the Supabase pooler.

The intended target is the dedicated `Postgres-v79y` service, not the existing
shared `Postgres` service used by OpenPaw. `foresight` should reference the
target database through Railway reference variables, for example:

- `DATABASE_URL=${{Postgres-v79y.DATABASE_URL}}`
- `DD_DBM_DATABASE_SERVICE=foresight-railway-postgres`
- `TEMPER_POSTGRES_MAX_CONNECTIONS` sized for the dedicated database and
  Foresight proof load

The storage backends remain Temper-native and Postgres-backed:

- `TEMPER_EVENT_STORE=postgres`
- `TEMPER_PLATFORM_STORE=postgres`
- `TEMPER_QUERY_PROJECTION_STORE=postgres`

No production data should be destroyed or overwritten as part of this change.
Before switching `foresight`, the operator must explicitly choose either a
fresh proof database or a preserving migration of the current `deep-sci-fi`
tenant data.

On 2026-06-18 the operator approved the fresh proof database path. Lane 4
switched only the dedicated `foresight` service to `Postgres-v79y`; Supabase was
left untouched as rollback source and no preserving migration was performed.

## Migration Options

### Fresh Proof Database

Use this when current Supabase `deep-sci-fi` data is not needed for the next
proof run.

1. Keep Supabase unchanged as rollback.
2. Point `foresight` `DATABASE_URL` at `${{Postgres-v79y.DATABASE_URL}}`.
3. Redeploy `foresight`.
4. Let startup run Postgres migrations and Genesis app reconciliation.
5. Verify `/healthz`, OData entity reads, tenant `deep-sci-fi`, and a fresh
   corridor action flow.

This was the selected path on 2026-06-18.

### Data-Preserving Migration

Use this when existing `deep-sci-fi` entities, sessions, trajectories, or
published workflow state must survive the switch.

1. Freeze or pause new `foresight` writes.
2. Take a logical backup from Supabase with `pg_dump` using the current
   `DATABASE_URL`.
3. Restore into `Postgres-v79y` with `pg_restore` or `psql`, depending on dump
   format.
4. Run read-only checks on row counts, tenant coverage, `_sqlx_migrations`,
   and selected known Foresight entity IDs.
5. Switch `foresight` to `${{Postgres-v79y.DATABASE_URL}}`.
6. Redeploy and verify the known tenant state plus a fresh action flow.

## Rollback

Rollback is variable-only as long as Supabase is left intact:

1. Set `foresight` `DATABASE_URL` back to the prior Supabase pooler URL.
2. Restore `DD_DBM_DATABASE_SERVICE=foresight-supabase`.
3. Redeploy `foresight`.
4. Verify `/healthz`, tenant reads, and that service logs show Postgres storage
   startup.

If a data-preserving migration was performed, keep the Railway database intact
for forensic comparison until the Supabase rollback is verified.

## Consequences

Foresight proof runs avoid the cross-provider Supabase pooler path and can
measure the remaining Temper/engine database workload more honestly.

Fresh proof mode is fastest and safest for infrastructure, but it starts from
an empty database and does not preserve existing `deep-sci-fi` run state.

Preserving migration keeps current data but requires an explicit write freeze,
backup, restore, and validation window. It must not be performed silently.

Datadog DBM tagging must be updated with the database move; the previous
`foresight-supabase` tag is no longer accurate after the switch.
