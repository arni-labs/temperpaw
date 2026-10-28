# Foresight engine

## Sub-features
World, EventNode, Path, Forecast, Hindcast, Dweller, Lens, Claim, Artifact, Endpoint.

## How to get to it (user POV)
A domain's future simulated as scored worlds; corridor architecture (backcast + constraints).

## Driving it
Create a World, dispatch Configure {endpoint_budget:1}, then Seed. Read back State=Seeding (the cheap deterministic proof). There is NO 'run' action; Endpoints are created by the sample_endpoints WASM on SampleEndpoints, never hand-seeded (a hand-created Endpoint is a dead entity - its advance actions are system-only).

## Gotchas
Runs are long and model-priced - endpoint_budget:1 and stop at the Created->Seeding read-back; a full corridor run is hours under SQLite contention. Seeding self-heals via ResumeSeed after 1200s. NOTE: APP.md documents the retired 0.1 surface (ForesightModel/Projection/etc, none exist) - trust the specs, not APP.md.
