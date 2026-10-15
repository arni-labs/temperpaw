# Spec: deterministic server deploy, verify, rollback

## Trigger

`on: push` to the `release` branch. `docker.yml` builds the image on `release`
(added to its push branches), then a `deploy` job runs `needs: build-and-push` and
`if: github.ref == 'refs/heads/release'`. Build and deploy are one workflow run, so
the image is guaranteed pushed before the deploy reads it. `main` still builds but
does not deploy.

## The driver

`stack/deploy/railway-deploy-verify-rollback.sh`, cloned at run time. Inputs by env:
Railway target ids and token, `BASE_URL`, `NEW_IMAGE_TAG` (`sha-<short7>` of the
release commit), `EXPECTED_SHA` (the full commit sha), `TEMPER_API_KEY`, poll tuning.

Sequence:

1. Capture last-good: the current Railway `IMAGE_TAG` variable and the live
   `/paw/version` `.sha`.
2. Upsert `IMAGE_TAG` to the new tag (with `skipDeploys`), find the latest
   deployment, `deploymentRedeploy` it.
3. Poll `/readyz` then `/paw/version` until `.sha == EXPECTED_SHA` or attempts run
   out.
4. On success exit 0. On failure restore the last-good tag (the exact prior tag; if
   the variable is unreadable, reconstruct `sha-<short7>` from the last-good sha),
   redeploy, re-verify, and exit non-zero.

## Invariants

- After the run, if the new deploy did not verify, the live service reports the
  last-good sha.
- The image tag deployed is `sha-<first 7 of the commit sha>`, matching
  `docker/metadata-action` `type=sha`.
- `EXPECTED_SHA` is the full commit sha, matching what the binary reports at
  `/paw/version`.
- Only one production deploy runs at a time (`concurrency` group, no cancel).

## Out of scope

The Genesis app publish/install path. TemperPaw's os-apps (and TemperPaw itself, a
Temper app) publish and install through Genesis via the native tool path
(`temper.publish_app` / `install_app`), the same path the TemperPaw agent uses.
Unifying that one path for every caller and making it deterministic with
rollback-on-fail is its own effort, not a separate CI reimplementation. The git-tree
clobber fix belongs to the katagami Genesis sync, not here.
