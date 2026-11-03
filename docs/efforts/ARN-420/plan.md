# Plan

1. Write the driver in `stack/deploy/railway-deploy-verify-rollback.sh`: Railway
   GraphQL helpers (read var, upsert var, latest deployment, redeploy), a verify
   loop against `/readyz` and `/paw/version`, capture-last-good, deploy, and
   rollback-on-failure. Done.
2. Prove it offline with a fake-`curl` harness: healthy deploy exits 0; a bad deploy
   rolls back to the exact last-good tag and exits 1; the sha-fallback works when the
   tag var is unreadable. Done.
3. Add `release` to `docker.yml` push branches and append a gated `deploy` job that
   clones the stack and runs the driver. Done (this PR).
4. Write the stack `deploy/README.md` documenting the two deploy paths and the wiring.
   Done.
5. Operational setup (outside this PR): create and protect the `release` branch; set
   `STACK_TOKEN` and the Railway secrets on the repo.
6. Live verification: merge a no-op to `release`, watch the deploy verify green;
   then a controlled bad image to watch the rollback. Pending secrets/branch setup.

## Expected end state

Merging to `release` deploys the server, verifies it live, and self-heals to the
last-good image on failure, with no hand steps.
