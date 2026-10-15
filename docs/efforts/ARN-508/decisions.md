# Decisions

## D1: Set the source image; do not template it

**Decision:** The workflow sets `source.image` to a literal tag via `serviceInstanceUpdate`.

**Came up because:** The alternative is to make the service source `ghcr.io/nerdsane/temperpaw:${{IMAGE_TAG}}` so the existing redeploy re-resolves the variable.

**Options:** Set the literal source from the workflow; template the source on the variable; keep upserting the variable.

**Chose the literal source because:** It matches how the service is already configured and how it was fixed by hand, and the running image is then readable from Railway's own record, which is what the assertion needs. A templated source hides the resolved tag behind a variable and makes "what is running" a two-step question again.

**Where:** .github/workflows/railway-redeploy.yml, "Set the service image".

## D2: Gate on Railway's deployment record, then /healthz; report /readyz

**Decision:** Success means a new deployment of the requested image reached `SUCCESS` and `/healthz` answers 200. `/readyz` is printed and warns on non-200; it does not fail the run.

**Came up because:** `/readyz` returns 503 whenever an optional integration is degraded. An expired Discord token made it 503 for days, and every workflow run failed at that step while the deploys themselves were fine.

**Options:** Keep gating on `/readyz`; gate on `/healthz` only; gate on Railway's record plus `/healthz`, report `/readyz`.

**Chose the last because:** Railway's record answers the question the workflow exists to answer, "is the requested image running", and `/healthz` proves the process behind it serves. Whether a degraded Discord connection should make the service not-ready is an application decision, not a deploy gate's; the warning keeps it visible. Keeping `/readyz` as a gate was rejected because it makes the workflow report failure for reasons unrelated to the deploy.

**Where:** originally "Wait for a new deployment" and "Verify the process serves"; after D5 the steps are "Wait for that deployment to succeed" and "Verify the process answers".

## D3 (superseded by D5): the version-endpoint policy, no longer in the workflow

**Decision:** When `expected_sha` is given: a sha that names a different build fails, whatever the tag. No usable answer - a non-200 (the endpoint is 503 today, ARN-508) or a 200 without a sha - warns for a `sha-*` tag, because the tag's hex is checked against `expected_sha` up front and Railway has confirmed that exact image deployed, so the proof already exists; it fails for `edge`/`latest`, because a mutable tag proves nothing about the build it resolved to. Revised twice: round three made a rejected call fail unconditionally, and the first live run with `expected_sha` then failed a successful deploy on a 503 from the version endpoint.

**Came up because:** `/paw/version` currently returns an empty body, so a required version match could never pass.

**Options:** Require the match; drop the check; warn on empty, fail on mismatch.

**Chose warn-on-empty because:** The image assertion already proves which build is running, so the version check is corroboration. An empty answer is a defect in the endpoint (filed on ARN-508), not evidence of a wrong deploy; a wrong answer is.

**Where:** same file, "Verify the process serves".

## D4: Deploy explicitly after setting the source; no wait-then-fallback

**Decision:** Call `serviceInstanceDeployV2` immediately after `serviceInstanceUpdate`. The earlier "wait six polls, then trigger once" fallback is removed.

**Came up because:** The first live run of the rewritten workflow showed the instance sitting for six polls after the source update with no new deployment and a null `latestDeployment.status`, until the fallback fired. Railway stages a source change; it does not deploy it.

**Options:** Keep the fallback (works, wastes a minute, and its comment claimed the opposite of what happens); always deploy explicitly.

**Chose the explicit call because:** It is the only path that deploys, so calling it a fallback was a false description of the mechanism, and the workflow should not carry a comment its own first run contradicted. The poll loop still asserts a deployment newer than the recorded one, of the requested image, in `SUCCESS`, so a double deploy would be harmless and a missing one still fails closed.

**Where:** .github/workflows/railway-redeploy.yml, "Start the deployment"; run 34759013602 is the evidence.

## D5: Subtraction after six rounds - keep what the live runs needed

**Decision:** The deploy steps are cut back to four: set the image, start the deployment and keep its id, wait for that id to reach `SUCCESS`, require exactly 200 from `/healthz`. The "before" snapshot, the superseded-deployment branch, and every `/paw/version` branch are removed. `expected_sha` is resolved at input time: accepted only with a `sha-*` tag whose hex it starts with, refused for `edge`/`latest`.

**Came up because:** Six panel rounds each found something real in the previous round's additions, and by round six the file had grown from 276 to 432 lines and codex's fix-it rubric failed it as over-engineered. Rita chose a subtraction pass over the arbiter or lifting the gate.

**Options:** Keep applying findings (rejected: the spiral); run the arbiter; lift the required check for one merge; strip back to what the seven live runs exercised.

**Chose subtraction because:** The runs proved four things matter - the image is set, a deployment is started and that specific one succeeds, and the process serves - and nothing else in the file was ever load-bearing. The "before" snapshot only fed the superseded-deployment branch, which binding to the started id makes unnecessary. Every `/paw/version` branch was reasoning about an endpoint that returns 503 and cannot corroborate anything today; refusing `expected_sha` for mutable tags up front says the same thing in one place. Round six's deferred findings on the version check and the pre-snapshot variable write disappear with the code they were about; the digest-resolution idea stays filed on ARN-508. What is given up: a version corroboration path that did not work, and a superseded-deployment message that a timeout now covers. 298 lines.

**Where:** .github/workflows/railway-redeploy.yml; crates/temperpaw/tests/temperpaw_identity_contract.rs (pins the tag-sha check, no longer the dead endpoint); docs/efforts/ARN-508/spec.md rewritten to describe this version.

## D6 (corrected by D7): keep DD_VERSION - built on a false premise

**Decision:** `BUILD_SHA` and `BUILD_VERSION` are never written by the workflow; `DD_VERSION` (and the OTEL `service.version`) are written from the expected sha when given, else the tag. The variable-delete helper is gone.

**Came up because:** After the subtraction (D5) the fix-it rubric kept failing on rounds seven to nine, and the arbiter (ASSESS-REVIEW-SPIRAL, run after four consecutive failures) traced every failure since round six to the build-identity block, which the intent never asked for. Its one question was whether anything downstream depends on the workflow writing those variables. Checked: `Dockerfile:66-67` bake `BUILD_VERSION` and `BUILD_SHA` as ENV from `docker.yml`; `DD_VERSION` is not baked and nothing else sets it, and Datadog reads it.

**Options:** Keep patching the block; delete all three variables and bake `DD_VERSION` into the image later; delete the two the image carries and keep `DD_VERSION`.

**Chose the last because:** Rita chose it. The two baked values can only be shadowed by a variable, never improved, so the workflow leaves them alone and the delete machinery that existed only to un-shadow them disappears with it. `DD_VERSION` has no other source, so dropping it would silently blank Datadog's version tag on every deploy. The contract test now forbids writing the two baked variables and requires `DD_VERSION`. What is given up: nothing that worked; on `main` the block wrote all three, and only when `expected_sha` was given.

**Where:** .github/workflows/railway-redeploy.yml ("Set Railway deployment variables"); crates/temperpaw/tests/temperpaw_identity_contract.rs; /tmp/assess-515.md (arbiter brief, fable).


## D7: The workflow writes no build identity at all; the image and its entrypoint own it

**Decision:** The workflow upserts only `IMAGE_TAG`. `BUILD_SHA` and `BUILD_VERSION` are baked into the image; `scripts/temperpaw-entrypoint.sh` already derived `DD_VERSION` from the baked `BUILD_SHA` and now derives `OTEL_RESOURCE_ATTRIBUTES` the same way when unset. `BUILD_VERSION` and `DD_VERSION` were deleted from the openpaw service once by hand (variableDelete, 2026-09-14). `OTEL_RESOURCE_ATTRIBUTES` stays on the service until an image with the new entrypoint is deployed; deleting it first would leave the running image with no service name in its telemetry.

**Came up because:** D6 stated that nothing else sets `DD_VERSION`. That was false: the entrypoint's line 6 sets it from `BUILD_SHA`, so the workflow's write replaced a per-build value with a tag. Fable found it in round ten. The check that missed it grepped the crates and the Dockerfiles and not `scripts/`.

**Options:** Keep writing `DD_VERSION` from the tag; write it from the expected sha only; write nothing and let the entrypoint own it.

**Chose nothing because:** It is the arbiter's literal recommendation, and the premise that made the alternative attractive was wrong. Identity that lives in the image cannot go stale and cannot be shadowed by a workflow that never writes it. The one gap - the OTEL attributes had no image-side source - is closed in the entrypoint with the same fallback shape as `DD_VERSION`. The contract test forbids all four identity writes. What is given up: a variable-level override of the version, which nothing needed.

**Where:** .github/workflows/railway-redeploy.yml ("Set Railway deployment variables"); scripts/temperpaw-entrypoint.sh:7; crates/temperpaw/tests/temperpaw_identity_contract.rs; the openpaw service's variables.
