# Durable Genesis DM Image E2E Proof — 2026-06-18

## Scope

Production had drifted back to stale Genesis app refs after a restart/deploy.
This proof records the durable repair path for Paw Discord DM image generation:

- publish refreshed Genesis app bundles;
- pin production bootstrap refs to those Genesis versions;
- redeploy production from the pinned refs;
- verify the runtime WASM hashes;
- exercise the Discord DM image-generation reply path end to end.

Production URL used for verification: `https://openpaw-production.up.railway.app`.

Railway project/environment/service:

- project: `ad7f8977-cf48-43ef-b129-ba1e17896ae4`
- environment: `production`
- service: `openpaw`

Linear issue: `ARN-61`.

## Source Merge

PR #413 was merged into `main`:

- merge commit: `5c3c05fd6292ef2eba9b2e6656ede5fd2fd896e9`
- PR: `https://github.com/nerdsane/temperpaw/pull/413`

The GitHub Docker workflow for that merge completed successfully:

- run: `https://github.com/nerdsane/temperpaw/actions/runs/27763518216`
- image tag: `sha-5c3c05f`
- result: `success`
- completed at: `2026-06-18T14:06:26Z`
- GHCR digest deployed by Railway: `sha256:44d80ae7f5dfc68c7b974408fd17e7c95918fb9a5cfc4f1c1b91de41086b89d3`

Because the functional fix is in Genesis-published WASM app bundles, production
was first repaired durably through Genesis refs, then the merged container image
was deployed for version parity.

## Genesis Versions Published

Published and verified refreshed app bundles:

- `temperpaw/paw-agent@9252bc166fe9106ef888b9a1e4ce4a432d063abf`
- `temperpaw/paw-media@6e82fed88dd0daed063e7b57332e20cc35b3e958`
- `temperpaw/paw-channels@79c34124892ab1a99f0de17bc544b0c0803192a0`

Bundle byte verification from Genesis:

| App | Module | SHA-256 |
| --- | --- | --- |
| `paw-agent` | `monty_repl` | `af3293660446b6ff5de831d4d02f149d49a0a8f253ce620696ecf364a41f1676` |
| `paw-agent` | `agent_reply` | `e36d025b52bf420b3e5a8fd1c6fb10a1a9eca157f253c683f492b7e80fe6495f` |
| `paw-media` | `openai_codex_image_generate` | `f5ba13c268f0cf639114922080502f5bda715c206c2188f1fb5f0baa91cdc67f` |
| `paw-channels` | `route_message` | `e90792f38e37d6ba25d0619c47667869a621c7e089a77dc01d0cd395535f8a97` |
| `paw-channels` | `send_reply` | `1ad697cf014c884767a8a91cba463693e532af5ae52dacb8af89ba344df4b123` |

Note: the dedicated Genesis repository `rp-temperpaw-paw-media` exists but is
stuck with `Status=Provisioning` and rejects `IngestPack`. The existing
`paw-media` app is bound to `rp-temperpaw-paw-agent`, so the durable media app
version was published to branch `paw-media-main-20260618-durable-pr413` in the
bound repository and verified through the normal
`/api/genesis/apps/temperpaw/paw-media/versions/<hash>/bundle` endpoint.

## Production Bootstrap Refs

`TEMPERPAW_GENESIS_BOOTSTRAP_REFS` was updated to include:

- `temperpaw/paw-agent@9252bc166fe9106ef888b9a1e4ce4a432d063abf`
- `temperpaw/paw-media@6e82fed88dd0daed063e7b57332e20cc35b3e958`
- `temperpaw/paw-channels@79c34124892ab1a99f0de17bc544b0c0803192a0`

The production variable was corrected after an initial shell escaping mistake
introduced literal `\/` in three refs. The stored value was re-read and verified
with normal `owner/name@hash` ref syntax before the successful proof redeploy.

Genesis-ref redeploy:

- deployment id: `773909a8-a6fd-497c-8d95-d0839c62a91f`
- image: `ghcr.io/nerdsane/temperpaw:sha-d7ce726`
- status: `SUCCESS`

Final image deploy:

- Railway service source updated from
  `ghcr.io/nerdsane/temperpaw:sha-d7ce726` to
  `ghcr.io/nerdsane/temperpaw:sha-5c3c05f`.
- first verified deployment id: `bfffe16a-4a5c-49cc-a265-24f73848b945`
- final verified deployment id after transport cleanup:
  `4abe83cc-f9cd-4cc8-9504-bc1529b55b9e`
- image: `ghcr.io/nerdsane/temperpaw:sha-5c3c05f`
- image digest:
  `sha256:44d80ae7f5dfc68c7b974408fd17e7c95918fb9a5cfc4f1c1b91de41086b89d3`
- status: `SUCCESS`
- healthcheck path: `/healthz`
- `/paw/version`: `{"version":"sha-5c3c05fd","sha":"5c3c05fd6292ef2eba9b2e6656ede5fd2fd896e9"}`

Note: updating only `IMAGE_TAG` was insufficient because the Railway service
source itself was statically pinned to the old image tag. The clean image path
was to update the service source image with `railway service source connect
--image ghcr.io/nerdsane/temperpaw:sha-5c3c05f`.

## Runtime WASM Verification

After the final image deploy, production `/observe/wasm/modules/*` returned:

| Module | Production SHA-256 |
| --- | --- |
| `monty_repl` | `af3293660446b6ff5de831d4d02f149d49a0a8f253ce620696ecf364a41f1676` |
| `agent_reply` | `e36d025b52bf420b3e5a8fd1c6fb10a1a9eca157f253c683f492b7e80fe6495f` |
| `openai_codex_image_generate` | `f5ba13c268f0cf639114922080502f5bda715c206c2188f1fb5f0baa91cdc67f` |
| `route_message` | `e90792f38e37d6ba25d0619c47667869a621c7e089a77dc01d0cd395535f8a97` |
| `send_reply` | `1ad697cf014c884767a8a91cba463693e532af5ae52dacb8af89ba344df4b123` |

This verifies the restart/deploy path no longer restores the stale hashes for
the DM image flow.

## Discord Transport Cleanup

After the image/WASM proof, `/readyz` surfaced a Discord transport retry residue:
runtime status was connected, but the persisted
`TransportConnection:transport-discord` entity was still `Retrying` with the
last error `Gateway bot endpoint returned 429 Too Many Requests: error code:
1015`. The entity had accumulated repeated `RetryDue -> StartRetry` events
while production was being redeployed.

Clean-up used the entity state machine, not database mutation:

1. Dispatch `TransportConnection.Disable` on `transport-discord`.
2. Wait past the previous retry interval so queued retry timers cannot advance
   the disabled entity.
3. Let startup/reconcile bring the transport back through
   `Configure -> Start -> StartSucceeded`.

Final transport readback:

```json
{
  "status": "Connected",
  "sequence_nr": 1095,
  "total_event_count": 1248,
  "attempt_count": 29,
  "last_error": "",
  "next_retry_at": "",
  "last_connected_at": "1781792845325",
  "interaction_url": "https://temperpaw.katagami.ai/discord/interaction"
}
```

Final `/readyz`:

```json
{
  "status": "ready",
  "healthz": "/healthz",
  "discord": {
    "status": "connected",
    "configured": true,
    "connected": true,
    "desired_state": "connected",
    "connection_state": "Connected",
    "last_error": null,
    "next_retry_at": null
  }
}
```

## Discord DM Image E2E

Dispatched a proof message through the active Discord DM channel:

- channel: `Channel:en-019eba01-164d-7d60-ab4d-5e710a5e39d6`
- action: `ReceiveMessage`
- thread/user: `1018228973869727785`
- proof tag/message id: `codex-durable-cat-20260618T135841Z`

Observed downstream state:

- session: `Session:ss-019edb06-e917-7122-ad95-cd21365b5def`
- session status: `Completed`
- media request: `MediaGenerationRequest:en-019edb07-05b9-73c1-8e5a-82c5c74bdb42`
- media status: `Complete`
- result file: `File:fl-019edb08-317b-7530-af93-efbae72c1ff5`
- result path: `/generated/images/en-019edb07-05b9-73c1-8e5a-82c5c74bdb42.png`
- MIME type: `image/png`
- provider response id: `resp_0fd0a7a8885eaf06016a33f99d588081919688c09571d5865a`

Session result included:

```text
Done — generated the cat image with the durable proof tag.

File: `/generated/images/en-019edb07-05b9-73c1-8e5a-82c5c74bdb42.png`

Proof tag requested: `codex-durable-cat-20260618T135841Z`
```

Reply attachment JSON:

```json
[{"file_id":"fl-019edb08-317b-7530-af93-efbae72c1ff5","file_version_id":"","filename":"en-019edb07-05b9-73c1-8e5a-82c5c74bdb42.png","kind":"pawfs_file","media_generation_id":"en-019edb07-05b9-73c1-8e5a-82c5c74bdb42","mime_type":"image/png","path":"/generated/images/en-019edb07-05b9-73c1-8e5a-82c5c74bdb42.png"}]
```

Channel delivery state:

- `SendReply:019edb08-4a49-7401-8274-981c214871d7`
- `ReplyDelivered:019edb08-4dfd-7a80-8567-f4ee5cb19172`

PawFS `$value` verification:

```text
/tmp/paw-cat-proof.png: PNG image data, 1536 x 1024, 8-bit/color RGB, non-interlaced
932d4d68b747e69ca26f5ba2c51758e37a8270fb0aeb812806ad81e611b93ad2  /tmp/paw-cat-proof.png
size: 2.3M
```

File metadata:

```json
{
  "status": "Ready",
  "fields": {
    "Status": "Ready",
    "mime_type": "image/png",
    "size_bytes": 2428398,
    "content_hash": "sha256:932d4d68b747e69ca26f5ba2c51758e37a8270fb0aeb812806ad81e611b93ad2",
    "has_content": true
  }
}
```

## Residual Risks / Follow-ups

- Repair the stuck dedicated Genesis repo `rp-temperpaw-paw-media` so future
  media app versions can publish to the canonical `paw-media` repository instead
  of the currently bound `paw-agent` repository branch.
- Update the deployment workflow or Railway setup so future image deploys update
  the service source image, not only `IMAGE_TAG`.
- The channel entity still carries an old `error` / `error_message` field from a
  historical 413 route failure, even though the new proof path completed. Clear
  or supersede stale channel error fields in a follow-up if they continue to
  confuse status surfaces.
