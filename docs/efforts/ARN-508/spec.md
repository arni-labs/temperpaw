# Spec

1. Validate the inputs. The tag is exactly `edge`, `latest`, or `sha-` plus
   7 to 40 hex, because it becomes a production source write. An
   `expected_sha` is accepted only with a `sha-*` tag whose hex is a prefix
   of it; `/paw/version`, which would corroborate a sha, returns 503 today
   (ARN-508), and a mutable tag proves nothing about the build it resolved
   to, so for `edge`/`latest` there is no proof to offer and the run refuses.
2. Write only `IMAGE_TAG`. Build identity is the image's: `BUILD_SHA` and
   `BUILD_VERSION` are baked, and the entrypoint derives `DD_VERSION` and
   the OTEL attributes from them. The workflow writes none of those.
3. Set the service instance's `source.image` to the requested tag
   (`serviceInstanceUpdate`), then start the deployment
   (`serviceInstanceDeployV2`) and keep the deployment id it returns. Railway
   stages a source change; it does not deploy on its own.
4. Wait for that deployment, by id, to reach `SUCCESS` with the requested
   image. A `FAILED`/`CRASHED` on that id fails the run at once; a poll that
   does not answer costs one attempt; no `SUCCESS` within the budget fails
   closed.
5. Require exactly 200 from `/healthz`. Print `/readyz` for the log; it folds
   optional integrations into its answer and does not gate a deploy.

Out of scope: rotating the Discord credential; whether a degraded optional
integration should make `/readyz` 503; making `/paw/version` return a sha;
the `temperpaw.katagami.ai` DNS; resolving a mutable tag to its digest.
