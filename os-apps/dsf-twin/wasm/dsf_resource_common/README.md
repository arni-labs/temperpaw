# Resource configuration and application verification

ResourceConfig version 3 requires `verification.application` in addition to its
flow and Datadog settings. There is no version 2 fallback.

An application binding is one of:

```json
{"kind":"railway","resource_id":"api-production","origin":"https://api.deep-sci-fi.world"}
{"kind":"vercel","resource_id":"web-project","origin":"https://deep-sci-fi.world"}
{"kind":"unbound"}
```

Unbound discoveries can collect observations. Every operation refuses an unbound
configuration before authorization reaches a provider. A later governed model
revision can supply the actual application relationship.

Railway deploy and rollback require the application resource to be the operated
service instance. Infrastructure configuration can name a related Railway API
resource; this includes a collector service that has no HTTP application itself.
The verifier reads that row and its config File, checks the exact File hash and
provider identity, and queries the instance's custom and service domains. The
requested origin must belong to that same project, service and environment.

Vercel operations require their own project. Production verification reads the
registered alias and requires it to route to the exact deployment in that
project. Preview verification uses the URL returned for that project's exact
revision. An alias change verifies the explicitly selected, registered alias.

Health and product probes use the proved origin. Datadog evidence must match the
successful health URL as well as its request ID, service, environment and full
revision. A production trace at the same revision cannot verify staging.
Authenticated operational snapshots remain restricted to the production API;
no production admin credential is sent to staging or preview.

The Railway domain response uses the official CLI GraphQL schema's
`serviceInstance.domains.customDomains` and `serviceDomains` fields. Each domain
must name the same provider identities and have `deletedAt: null`. Reads are
bounded to 100 domains in each collection. The source is
[Railway's CLI schema](https://github.com/railwayapp/cli/blob/master/src/gql/schema.json).
