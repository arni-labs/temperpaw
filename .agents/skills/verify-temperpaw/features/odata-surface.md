# Governed OData surface

## Sub-features
Entity reads (`GET /tdata/<Set>`), action dispatch (`POST /tdata/<Set>('<id>')/Temper.<Action>`), Cedar authorization, tenant scoping.

## How to get to it (user POV)
Agents and tools read entities and dispatch governed actions - this IS the platform's API.

## Driving it
Bearer `$TEMPER_API_KEY` + `X-Tenant-Id: default`. Read a set, dispatch an action, read the entity back.

**The key only works if it was in `.env` at boot** - the platform bootstraps a tenant credential from `TEMPER_API_KEY` during startup. A keyless boot serves 401 on every `/tdata` route; adding the key after boot does nothing until restart.

## What proves it
The state machine moved: the entity's state field after the action matches the spec's transition. A denial proves governance: an unauthorized dispatch returns 403 (and per stack rules, the denial surfaces - silent 403 handling is a finding).

## Gotchas
The service document (`GET /tdata/`) returns 401 even with a valid key - probe a concrete entity set (`/tdata/SkillPackages`) instead. Set names come from the spec entity names pluralized.

A 200 on dispatch is not a transition - always read back. Cross-entity guard arrays 409 on stringified JSON (pass real arrays). Fields over 32KB truncate - use file refs.
