# Plan
1. Set READY_PATH=/healthz on the deploy job (this PR).
2. Rerun the release deploy; expect verify to pass and sha-7fcfae7 live.
3. Separate follow-up on ARN-432: readyz decoupling in the app + fix the
   Discord 401 credential.
