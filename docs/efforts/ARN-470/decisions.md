# Decisions

### Preserve truthful review status

**Decision:** Install Stack’s distinct exact-head owner disposition in the Temper-backed review gate.
**Came up because:** Explicit no-further-review authorization could not be represented without inventing a ReviewRun for an unreviewed release.
**Options:** Fabricate review evidence; silently disable review checks; use a separate explicit owner disposition.
**Chose the explicit disposition over fabricated or missing review because:** The explicit disposition because normal review and proof remain in force and the gate reports the actual review status.
**Where:** .github/workflows/sdlc-review.yml and .github/workflows/sdlc-decision-intake.yml.
