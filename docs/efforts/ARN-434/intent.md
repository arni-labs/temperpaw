# ARN-434: Cedar permits for the shadow-sweep writes

## Problem
The ARN-431 S1 acceptance run surfaced the ARN-430 residual: the shadow sweep's
prod principal is Cedar-denied for the record-write actions. `ReviewRun.Ingest`,
`ReviewRun.IngestRecord`, `ProofPacket.Ingest`, `ProofPacket.IngestProof`, and
`ShadowVerdict.Record` / `MarkAgree` / `MarkDisagree` all return
`403 AuthorizationDenied: no matching permit policy`. So entities lazy-create but
never advance, and the sweep can only produce `na` for review/proof.

## Proposed outcome
paw-patrol's `patrol.cedar` grants EXACTLY those actions on EXACTLY those three
resources to EXACTLY the principal the sweep authenticates as, and nothing more.
After publish+install, the sweep's Ingest advances ReviewRun/ProofPacket to
Recorded and its ShadowVerdict writes populate the rows - review/proof become
real entity-derived verdicts.

## The principal (determined empirically, not guessed)
The key the sweep uses (temperpaw `secrets.TEMPER_API_KEY`) is byte-identical to
the prod deployment's operator key (openpaw service `TEMPER_API_KEY`; verified by
sha256). That key bootstraps as the operator `AgentCredential` (AgentType name
"operator"), so it resolves to `principal is Agent` with
`principal.agent_type == "operator"` and `principal.agentTypeVerified == true` -
the same verified-operator predicate `seed_operator_manage_policies` already uses
in the kernel. Requiring `agentTypeVerified == true` scopes the grant to the
credential-authenticated operator, never a self-declared (header) agent.

## Affected users and systems
paw-patrol `policies/patrol.cedar` only. No entity, module, or workflow change.
After merge: publish to Genesis + install on prod (the lead's approved pattern),
then rerun the ARN-431 acceptance sweep.

## Constraints
- Narrow: exactly the 6 denied action names on exactly the 3 resources, gated on
  the verified operator. No broadening of any existing permit.
- Red-green: a probe showing the denial BEFORE and the allow AFTER, captured.
- One PR (temperpaw). No panel, no merge (the lead owns those).
