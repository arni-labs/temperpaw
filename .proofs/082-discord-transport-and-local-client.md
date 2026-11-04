# Proof Report: 082 — Discord transport start, this server's own API client, transport owners and dashboard sessions

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
- A transport takes a message, command or press only from an owner of its bot,
  checked in one place, `paw_transport::owners::Owners::admit`. The moves those
  lead to ask for the `Owner` proof it returns, so a later transport has to pass
  through it. Owners come from the provider: Discord's application record (the
  owner, or a team's owner and accepted admins) and Slack's user record (a
  workspace owner). When the provider gives no clear answer nobody is admitted,
  and a refused sender gets a plain answer.
- The Discord and Slack press handling is one function, `apply_press`, and
  Discord interactions are always signature-checked.
- A dashboard session reaches the kernel's declared routes: the edge presents a
  single-use capability bound to that request and naming the signed-in person.
- TemperPaw seeds one Cedar permit at boot, `temperpaw-owner-approvals-and-reads`,
  giving an Admin `manage_policies`, `read_events`, `read_agents`,
  `read_app_catalog` and `write_trajectories`. An Admin here is the one account
  this server registers or one of its own in-process identities. The dashboard's
  approvals, app list and live events need these, an owner's press on a decision
  does too, and a session a transport started needs the last to record its work.
- The first account takes the operator credential. While a server has no account,
  `/auth/register` answers 401 with a plain message unless the request presents
  it, and the login page asks for it. Registration still closes after the first.

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

Dashboard, step 3: register, then open every page headless and record what its
kernel reads answer. The walk answers `/paw/setup/status` in the browser so the
first-run redirect doesn't send every page to `/welcome`.

| Page | main: kernel reads, 401s | this branch: kernel reads, 401s, 403s |
|---|---|---|
| `/` | 10, 10 | 11, 0, 0 |
| `/agents` | 3, 3 | 4, 0, 0 |
| `/agents/new` | 1, 1 | 2, 0, 0 |
| `/apps` | 2, 2 | 3, 0, 0 |
| `/apps/[name]` | 1, 1 | 2, 0, 0 |
| `/connections` | 1, 1 | 2, 0, 0 |
| `/entities/[type]/[id]` | 4, 4 | 5, 0, 0 |
| `/permissions` | 4, 4 | 5, 0, 0 |
| `/platform` | 3, 3 | 4, 0, 0 |
| `/project/[id]` | 4, 4 | 5, 0, 0 |
| `/sessions` | 2, 2 | 3, 0, 0 |
| `/sessions/[id]` | 5, 5 | 6, 0, 0 |
| `/settings` | 1, 1 | 2, 0, 0 |
| `/welcome` | 1, 1 | 2, 0, 0 |

Screenshots of `/permissions` and `/apps` show the policy registry and all 16
apps. The 404s left are made-up ids in the walk and `/tdata/Skills`,
`/tdata/Harnesses` and `/tdata/ProjectHarnesses`, which the local server does not
serve. `/paw/infra/updates` answers 502 locally; it is not a kernel read.

## Tests

- `a_press_through_discord_decides_a_real_decision_only_for_an_owner`: signed
  presses through the Discord handler, the client, the deployed edge and the
  kernel, on a real pending decision.
- `an_owners_press_decides_a_real_decision`: deny and each approval scope.
- `the_first_account_takes_the_operator_credential`: nobody, a made-up key, an
  agent's credential and an internal capability are refused and leave registration
  open; the operator credential creates the account; afterwards registration is
  closed with the credential or without.
- `a_signed_in_person_reaches_the_kernels_routes`: 401 with no session, 200 with
  one, and a route a tenant declared unchanged.
- `owners::tests`, `discord::transport::tests`, `slack::transport::owner_tests`:
  the rule, each provider's record, and each way in.
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
- An owner is who the provider says owns the bot. A Discord application owned by
  another account, or a Slack workspace with another owner, refuses the person
  running the server until that is put right. Slack's check needs the bot's
  `users:read` scope and refuses everyone without it.
- The kernel records the client's identity, `temperpaw-transport`, as who decided.
  The message shows who pressed, and so does the transport's log.
- The owner's press on a real decision was exercised with a signed request, not a
  press in Discord itself.
