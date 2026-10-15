# Provider-owned resource facts

Status: accepted for ARN-467.

Deployment providers and Datadog report different revisions. Railway and Vercel
identify deployed source revisions; DSF's Datadog instrumentation currently uses
static version tags. Applying both sources to the same configuration fields could
replace a deployed commit with a tracer version or an empty value.

Model Datadog service telemetry as a distinct resource linked to the deployment
resource. Its ModelSync writes observations only to that telemetry resource.
The deployment provider remains the owner of deployment configuration and revision
facts. Flow dependencies connect both resources where the user needs their combined
outcome.

This avoids adding source-specific projection branches to every resource action.
The adapters must validate their configured source/resource binding before returning
facts. A collector cannot redirect its result to another resource: the sync record
retains its resource ID from Configure and later callbacks do not accept a new ID.
