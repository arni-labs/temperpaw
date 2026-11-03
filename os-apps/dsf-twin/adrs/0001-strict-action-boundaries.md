# Strict action boundaries for factory records

Status: accepted for ARN-467; installation requires the corresponding Temper
runtime support.

The pinned runtime at `d7a48b92f7caf724067972640c0cfc302f6a350e` merges undeclared
action parameters into entity fields after evaluating guards. Declaring
`Observe` without an `intended_configuration` parameter therefore does not stop
a caller from replacing that field. The production actor simulator reproduced
the same failure for operation targets and observation replay.

Factory specs opt into strict action parameters. The runtime rejects undeclared
parameters and checks declared parameter constraints against the pre-transition
record before any mutation or integration trigger. Generic field writes cannot
change strict records; creation admits only identity and initial status. State
changes use the declared actions.

Observation updates compare the expected sequence and require a newer timestamp.
Operation callbacks match accepted keys and targets. Experiment validation
compares real production bindings with the configured candidate. Immutable
observation records have no transition after recording.

Cedar remains responsible for who may invoke an action, including integration
callbacks and accepted decisions. It does not replace atomic parameter validation
in the evaluator. Provider adapters validate external evidence and redact secrets.
This keeps authority checks, atomic record checks and provider validation explicit.

The alternative was to validate fields in each integration after the transition.
That would allow an invalid request to mutate the target before validation and
would leave generic writes unprotected. The runtime change is required before
this app can satisfy its contract; passing parser-only tests is insufficient.

Declared defaults must exist before the first action. The runtime projects typed
counter state into fields; accepting a counter parameter does not assign it.
ModelSync and Observation therefore use explicit `set_counter_from_param`
effects for timestamps and resource sequence values. The contract suite checks
that every accepted counter value sharing a declared counter name has an
assignment effect. Fresh records no longer use decrement operations to create
zero-valued counters.
