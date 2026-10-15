# Plan

1. Replace "Find current deployment" + "Trigger redeploy" + "Verify" with the four steps in the spec.
2. Validate the read query's shape against the live service with a personal token (read-only).
3. Run the workflow from the branch with the already-deployed tag: proves set, wait, assert, and the health gate end to end with a same-image redeploy.
4. Panel, then a human presses merge (workflow change).
