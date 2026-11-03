# ARN-427: gate-quality wave

## Problem
The vendored SDLC gates shipped with rough edges found by their own panel
rounds: a plain-text record marker that a review finding could truncate,
unpaginated comment reads, an unserialized intake, and a checkout action
version the repo's runtime smoke now rejects. Each one either fails a PR
falsely or lets a record go unread.

## Proposed outcome
Gates on this repo byte-identical to the fixed stack source, and a green
checks job on every PR again.
