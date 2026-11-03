# Bind operation configuration and verify observed outcomes

Status: accepted for ARN-467.

Every operation pins a Temper configuration File ID and the SHA-256 of its exact
bytes. All four integrations reread and hash that File. This lets the operation
carry provider identities, selected jobs and probe expectations without allowing
later File edits to change an accepted target.

Routine deployment uses the linked Effort's existing authorization, proof and
review, the resource's allowed operations and Cedar. Required Asks apply only to
actual recorded choices; the default list is empty. Adding a mandatory deployment
approval would contradict autonomous delivery of authorized work.

Provider receipt, exact runtime revision, affected-flow response and matching
Datadog span are separate checks. Provider creation alone cannot become Verified.
A missing result or late span remains pending and retains the resource lock.
Bounded retries stop with the unresolved state visible. Partial media claims or a
failed job cannot release a reservation while another claimed job still runs.

Railway's deployment mutation has no verified provider idempotency token. The
adapter correlates the exact target, SHA, baseline and operation time; after an
ambiguous send it reads provider state and never sends a second deployment attempt.
Vercel adds operation metadata. DSF media recovery provides a durable idempotent
receipt keyed by the operation UUID. These are different provider contracts;
the adapter does not claim a universal exactly-once provider guarantee.

The implementation's additional-spend cap is tracked by the effort owner. Deploys
use existing allocations. Media configuration pins selected-job ceilings and
checks estimates before submission and recorded costs afterward. This does not
create a global reservation system or a provider-enforced financial cap. New paid
provisioning belongs to the experiment path with its own cost authority.

State-timeout occurrence budgets match their action counters: three execution
attempts, twenty observation attempts and forty verification attempts. The
runtime defaults each timer to one firing, so omitting this declaration would
stop unattended retries before the action budget was used. The contract suite
checks these timer budgets alongside the actual actor retry guards.
