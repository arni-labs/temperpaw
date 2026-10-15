# ARN-420: deterministic production deploy with auto-rollback

## Problem

Deploying the TemperPaw server to production is manual and has no rollback.
`railway-redeploy.yml` is a `workflow_dispatch` a human runs, and if the new image
comes up broken there is nothing that puts the service back — someone has to notice
and redeploy the old tag by hand. That is the last open gap in the SDLC loop:
everything up to merge is gated, but the deploy itself is a hand operation that can
leave prod down.

## Proposed outcome

Merging to a `release` branch deploys automatically. The deploy captures the
currently-live image first, deploys the release commit, verifies the live service
reports that commit, and if verification fails restores the last-good image and
re-verifies. A failed deploy ends with prod back on its last-good image, not down.

## Affected users and systems

- The server on Railway (GHCR image pulled by Railway).
- `docker.yml` (image build) gains a gated `deploy` job.
- Deploy/verify/rollback logic lives in `arni-labs/stack` (`deploy/`), cloned at run
  time — same pattern as the SDLC gates.

## Constraints

- No clobber path: the server deploys as an immutable image tag; nothing here does a
  git push to Genesis.
- Never cancel a deploy mid-flight.
- A stale release (behind main) must not deploy.

## Open questions

- TemperPaw is itself a Temper app: its os-apps publish and install through Genesis
  (hash-pinned), and the Railway server loads those pinned refs. Publishing and
  installing goes through the native Genesis path — `temper.publish_app` /
  `install_app` — the same path the TemperPaw agent uses and the same path any other
  agent, CI, or human should use. Genesis is the source of truth; the front door
  should be one path, not forked per caller. Making that Genesis publish/install
  deterministic and adding rollback-on-fail is a real but larger piece. Whether it
  belongs in this effort or a tracked follow-up is a scope call for the owner. This
  effort ships the server (Railway) path.
