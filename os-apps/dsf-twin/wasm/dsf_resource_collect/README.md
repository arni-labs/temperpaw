# Typed resource observations

Each generated collector selects one Rust type: `Railway`, `Vercel`, `Supabase`,
`R2`, `Datadog`, or `Media`. Its corresponding action target supplies the registered
provider identity and named credentials. There is no provider selector or write
API in this crate.

`collect::<C>` checks the captured refresh sequence, rereads the exact resource
row, loads its ResourceConfig v3 File, verifies the File hash, and validates the
provider target. Provider reads then produce one of the declared
`CollectionMeasured`, `CollectionAbsent`, `CollectionInaccessible`, or
`CollectionStale` callbacks. Every callback carries the original refresh sequence
and the actual observed sequence read before collection.

IOA first creates an immutable observation and then projects it through the
resource's observation CAS. A later conflicting observation can refuse that
projection without discarding the recorded evidence. Collection never adopts
intended configuration.

| Type | Read evidence |
| --- | --- |
| Railway | Exact project/service/environment, selected service settings, latest deployment, and bounded active deployments. The running revision comes only from one active successful deployment. |
| Vercel | Exact project/team, selected build settings, and the production deployment pointer. |
| Supabase | Exact project metadata and selected PostgreSQL settings. |
| R2 | Exact bucket metadata and CORS policy. |
| Datadog | The real monitor ID, query, options, and provider state. This is monitor configuration, not a metric count. |
| Media | Current DSF media queue counts and at most 20 job references, with echoed limits, has_more, revision, and source time. |

Stored evidence omits environment-variable values, credentials, arbitrary response
fields, monitor messages, and product content. It includes the source URL/query,
time window, sample kind, bounded facts, and coverage. Media snapshots older than
60 seconds are stale. A missing DSF endpoint is inaccessible; it does not prove
that the application is absent. Permission and malformed-response failures never
count as absence. Empty queues remain measured zeros.

Generated WASMs call `guest::run::<C>()`. The public HTTP host interface supports
recording fixtures, and the actor contract suite drives real collector callbacks
through Temper's production actor evaluator and declared reactions. Provider
fixtures and actor tests do not substitute for live verification of deployed
credentials and provider schemas.
