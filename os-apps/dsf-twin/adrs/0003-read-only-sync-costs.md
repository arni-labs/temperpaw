# Read-only sync costs

Status: accepted for ARN-467.

The initial ModelSync draft included a reservation integration without a durable
reservation record. A read-only module cannot reserve shared funds by checking a
cap, so that phase would have claimed a guarantee it did not enforce.

ModelSync performs bounded provider reads on the existing service. It starts no
computers, agents, model calls or paid product generation. Refresh invokes the
collector directly, and declared scheduling controls the interval and retries.
Service hosting is included in the deployment budget.

Paid computer work, experiments and product verification require an actual budget
reservation during operation validation. Those operations remain subject to the
authorized implementation cap, including committed costs. This decision removes
the unsupported per-read reservation phase without authorizing additional spend.
