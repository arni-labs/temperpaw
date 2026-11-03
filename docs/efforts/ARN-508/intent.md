# ARN-508: the deploy workflow must actually change the image, and must prove it

The Railway redeploy workflow upserted an `IMAGE_TAG` variable and then
redeployed the previous deployment. The service's source is a literal image
reference, so the redeploy replayed the old image and nothing read the
variable: for two days the variable said `sha-f2eb7d1` while production ran
`sha-c3689f0`. Its readiness gate polled `/readyz`, which had been 503 for
days because a Discord credential expired, so every run also reported
failure. Found while deploying the ARN-467 kernel fix by hand.
