# App install from Genesis

## Sub-features
`POST /paw/apps/install-from-genesis`, pinned refs (`owner/app@hash`), Cedar activation for the installed app's entities.

## How to get to it (user POV)
An operator installs or updates a platform app from the Genesis registry.

## Driving it
POST the install endpoint with the app ref; then list the installed apps and read one of the new app's entity sets.

## What proves it
The installed pinned ref matches what was published, and the app's entity set answers (not 403 - see the known activation gap, ARN-164). Production shape: verify on the deployed shelf per the Definition of Done, not just locally.

## Gotchas
GENESIS_TOKEN (not GENESIS_API_KEY). Install does not GC superseded Cedar (ARN-399) - policy tightening needs a check that old permits are gone.


## Corrections (maintain pass 2026-08-26)
- No 'list installed apps' surface: prove via the install response body (app_ref, closure_id, materialized_apps, wasm_modules) and provenance rows.
- GENESIS_TOKEN belongs to publish/sync, not install (install fetches unauthenticated).
- Pass registry_url EXPLICITLY: omitting it silently targets production Genesis. Git materialization fallback is off unless TEMPER_GENESIS_INSTALL_GIT_FALLBACK is set.
- Re-POST the same ref -> skipped (idempotence proof).
- follow_policy is pinned (default) or follow_latest; only pinned proves a hash match.
- SECURITY (ARN-410): this endpoint does not authenticate - a wrong bearer key with a valid body reaches install logic (500 on unreachable registry, not 401). Confirmed live.
