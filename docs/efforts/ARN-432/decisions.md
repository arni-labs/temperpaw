## Decisions & Tradeoffs
**Decision:** Point the pipeline's readiness poll at /healthz instead of /readyz (interim).
**Came up because:** readyz is chronically 503 on a Discord 401, so verification can never pass on any image.
**Options:** fix readyz first (rejected: app change through the full loop while deploys stay broken); poll healthz now.
**Chose healthz because:** identity is already proven by the /paw/version sha and Railway's image record - readyz added only liveness, and healthz is liveness without the optional-integration coupling. Given up: catching genuine core-unreadiness that healthz misses, until the readyz fix lands.
**Where:** .github/workflows/docker.yml deploy env; ARN-432 tracks the real fix.
