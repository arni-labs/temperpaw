## Decisions & Tradeoffs

**Decision:** Fold the checkout bump, the gate re-vendor, and the fmt fix
into one PR.
**Came up because:** each alone left the checks job red or the gates unable
to read current records; three PRs would each need their own panel round.
**Options:** three separate PRs (rejected default); one wave.
**Chose one wave because:** the pieces gate each other - a green checks job
needs both the checkout bump and the fmt fix, and a readable record needs
the re-vendor. Given up: a finer-grained history.
**Where:** this branch; stack source commits ba36d2b, 1975d37, 1b4a6b1.
