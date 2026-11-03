# ADR-033: Temporary Media Route Monty Hot Upload

- Status: Accepted
- Date: 2026-06-17

## Context

The production `temper.image_generate` tool is owned by `monty_repl`. The clean deployment path is for Genesis to publish and pin a new `paw-agent` app ref containing the updated `monty_repl.wasm`.

During the media runtime reconcile, Genesis Git accepted small app publishes but the full `paw-agent` publish path stalled or failed while transferring the large pack containing `monty_repl.wasm`. Retrying with a minimal overlay branch containing only the 6.6 MB Monty module failed with the same Git protocol error.

Production still needed the updated Monty route immediately so DMs would create `MediaGenerationRequest` entities through `/tdata/MediaGenerationRequests` instead of the legacy `/tdata/MediaGenerations` route.

## Decision

Until the Genesis large-pack publish issue is fixed, production may hot-upload the rebuilt `monty_repl.wasm` module after a redeploy with `POST /api/wasm/modules/monty_repl`.

The hot-uploaded module must be verified through `/observe/wasm/modules/monty_repl` and its hash must match the local release artifact. The 2026-06-17 production media reconcile verified hash:

```text
a09f51290b777da7839919507c836b2c71deb1b315000696808c53c8568a8049
```

This is a temporary deployment bridge, not a new orchestration layer. Runtime behavior remains Temper-native: Monty creates the entity, dispatches `Generate`, and the media/provider WASMs advance the workflow by actions.

## Consequences

Production DMs can use the corrected app-scoped media route before the full `paw-agent` app ref is publishable.

App reconciliation or a redeploy can replace the hot-upload with the older Genesis-pinned Monty module. Operators must either re-upload and verify the module after redeploys or complete the preferred fix: repair Genesis large-pack publishing and pin a new `paw-agent` ref that contains this Monty module.

The temporary state should be removed once `temperpaw/paw-agent` is published with the updated `monty_repl.wasm` and production `TEMPERPAW_GENESIS_BOOTSTRAP_REFS` points at that ref.
