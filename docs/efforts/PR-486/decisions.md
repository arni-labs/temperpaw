# Decisions & Tradeoffs

**Decision:** Select Temper proof packets by the exact PR commit before evaluating their content.

**Came up because:** The unfiltered collection is limited to100 rows and omitted the real Recorded packet for this release.

**Options:** Increase an arbitrary row limit, duplicate or reorder evidence, or use the supported commit filter.

**Chose the commit filter because:** It retrieves this release's evidence without changing proof requirements or unrelated records. The same approach is already used by the repository's review gate.

**Where:** `.github/workflows/sdlc-verification.yml`; the exact workflow query fails against100 unrelated fixture rows before the fix and succeeds after it. Live filtered read returns the real Recorded packet.
