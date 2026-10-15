# ARN-432: deploy verification must not depend on a degraded optional integration

## Problem
The first live governed deploy failed verification for a healthy image and
then declared its (healthy) rollback failed too, because /readyz couples the
optional Discord integration into readiness and Discord has a 401.

## Proposed outcome
Interim: the pipeline verifies on /healthz + the /paw/version identity check.
Real fix (same issue, later effort): /readyz gates on core readiness only.
