# Selected media recovery

`RetrySelected` implements one action on `DsfMediaPipeline`. It reads and repairs
only the 1–20 generation IDs in the accepted resource action. It does not select
jobs itself or invoke either endpoint's unrestricted batch behavior.

The registered target contains `application_id`, `environment_id`,
`api_resource_id`, `bucket_resource_id`, and `token_secret`. These identities must
match the resource. Here `environment_id` is the DSF application environment
label and must be `production`; it is distinct from a Railway environment UUID.
Even a matching preview row and configuration cannot use this production API.
All DSF API requests use `https://api.deep-sci-fi.world`.

Before the repair POST, the adapter resolves the linked `DsfRailwayServiceInstance`
and `DsfCloudflareR2Bucket`, verifies their configuration File hashes and exact
provider identities, then reads their provider domain bindings. The Railway
service must own `api.deep-sci-fi.world` in the registered project/environment.
The R2 bucket must serve `media.deep-sci-fi.world` with enabled access and active
ownership and SSL. A label alone cannot establish either binding. These checks
use the Railway CLI GraphQL schema and the [Cloudflare custom-domain API](https://developers.cloudflare.com/api/resources/r2/subresources/buckets/subresources/domains/subresources/custom/methods/list/).

The change contains `generations`, `max_cost_cents`, and `cost_authority_ref`.
Each generation contains its UUID, `target_type` (`world` or `story`), target UUID,
`media_type` (`cover_image`, `thumbnail`, or `video`), and `max_cost_cents`. The
ordered IDs and authority reference must match the accepted action fields.

The shared runtime verifies the Effort, exact change proof, and required Asks.
This adapter also requires `cost_authority_ref` to name a required, answered Ask
on that Effort. Its recorded choice must contain a positive integer
`max_cost_cents` and `agent_auth: subscriptions_only`. The sum of selected job
ceilings must fit both the change and that authority. This check authorizes one
operation; it does not account for cumulative task spending. No Ask is created
by this module.

The provider operation UUID is derived from the resource ID, operation key, and
sequence. Replays use the same UUID; another sequence cannot adopt the earlier
receipt. Execution first reads `/api/media/recovery-operations/{operation_id}`.
After a confirmed 404 it checks the current API revision and each selected job,
then calls `/api/media/retry-stuck` with the UUID and exact IDs. A job already
owned by this UUID without a readable receipt remains uncertain. The adapter
does not repeat it.

Preflight checks the actual target and media type. Image generation costs two
cents. Video generation costs five cents per second for 5–15 whole seconds.
The actual duration must fit the selected job's ceiling before any POST.

Receipt validation checks the outer and inner operation IDs, selected IDs,
unique outcomes, queued count, endpoint, and original response shape. It retains
partial claims. Verification waits for every claimed job to settle before
reporting a failure, including when another selected job was refused.

For success, every selected job must have completed under this attempt. The
adapter reads the cost, checks the artifact's generation/attempt path on an
allowed HTTPS media host, and sends an unauthenticated HEAD request. It then
checks current API health at the exact revision and the matching Datadog probe
span. Verification uses at most 46 HTTP calls for 20 jobs: one receipt, 20 status
reads, 20 artifact reads, three application resource/config/domain reads, one health probe, and one Datadog query.

Tests use the shared runtime and recorded HTTP requests. They cover ambiguous
receipts, replay identity, partial work, price and authority boundaries, and the
full artifact/health/Datadog sequence. They make no paid provider calls. Generated
stage modules still require actual WASM-engine and authorized live verification
before release.
