# Proof Report: 082 — Discord transport start and this server's own API client

## Date

2026-11-04

## Branch / Commit

- Repository: `arni-labs/temperpaw`
- Branch: `claude/openpaw-discord-webhook`, from `20c618f53`
- Kernel pin: Temper `d1970395ea68`

## What Was Done

- The Discord reconcile moves (`Configure`, `Start`) now run as an Admin named
  `transport-reconcile` instead of the System context. The kernel issues a WASM
  module no internal capability for a System move, so `transport_reconcile`'s
  call failed before it was sent, and its retry timer inherited the same
  identity.
- `PawApiClient` takes a per-request credential. `auth::local_api_client` mints
  a single-use internal capability for each request, naming
  `temperpaw-transport` as an Admin. The Discord and Slack transports, the
  Discord observer and the webhook trigger use it.
- `TransportManager` no longer holds the API key.

No ADR: this applies the credential direction ADR-0157 already set.

## Why Not the API Key

The operator credential resolves to kind Agent. `channels.cedar` and
`webhook.cedar` permit these writes to an Admin, so the key reads channels and
is then refused on the moves that matter. Local run: the operator answers 403
on `/tdata/WebhookEvents`, while the client's signed delivery above it is
accepted.

## Verification Flow

Throwaway local server, fresh database, the same script for both binaries. The
Discord token and the webhook secret are made up for the run.

1. `DISCORD_BOT_TOKEN` set to a junk value, boot, read `/readyz` after 45 s.
2. Store a secret for `patrol-request`, POST one correctly signed delivery and
   one with a wrong signature to `/triggers/webhook/patrol-request`, and read
   the `WebhookEvent` transitions from the log.

## Evidence

Discord, step 1:

| Binary | `connection_state` | `last_error` |
|---|---|---|
| main | `Retrying`, repeating every 30 s | `HTTP call failed: POST http://127.0.0.1:4731/paw/internal/transports/discord/start` |
| layer 1 only | `Failed` | `query Channels returned 401 Unauthorized:` |
| this branch | `Failed` | `Gateway bot endpoint returned 401 Unauthorized: {"message": "401: Unauthorized", "code": 0}` |

The last row is Discord refusing the junk token: every local step before the
gateway now passes.

Webhook, step 2:

| Request | main | this branch |
|---|---|---|
| Signed delivery, secret stored | 503 `webhook route lookup failed` | 200 `accepted` |
| Wrong signature | 503 `webhook route lookup failed` | 401 `webhook signature verification failed` |
| `WebhookEvent` transitions | none | `Received`, `Routed`, `Processed` |

## Tests

- `this_servers_own_client_passes_both_edges`: the deployed edge on a real
  socket, with an unminted client as the control.
- `a_minted_credential_is_presented_for_exactly_the_request_it_names`.
- `a_discord_reconcile_move_can_call_this_server_and_is_admitted`.

## Limits

- No real Discord connection yet: that needs a real bot token. Writes after the
  gateway is ready (channel creation, message routing) use the same client and
  identity as the webhook writes above, but were not exercised on Discord.
- Local runs load apps from `os-apps/`. A deployment uses its Genesis pins; the
  policy this relies on last changed in the repo on 2026-09-30.
- `temperpaw tui` on loopback still uses the old client path and was not changed.
