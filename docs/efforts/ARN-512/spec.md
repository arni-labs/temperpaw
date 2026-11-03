# ARN-512 spec (temperpaw side) — pin the kernel that keeps schema annotations

The kernel fix is two PRs: nerdsane/temper#470 (`90548bd1`, schema-level `<Annotation>`) and nerdsane/temper#472 (`bd15e892`, targeted `<Annotations Target=…>` blocks). The pin follows the later one. temperpaw pins the kernel
by git rev in `crates/temperpaw/Cargo.toml` and `crates/paw-codex-worker/Cargo.toml`;
the openpaw image is built from this repo, so the fix reaches production only
through this pin.

Contract: the pinned kernel emits schema-level annotations and targeted
annotation blocks in `$metadata`; after the image ships, production `$metadata`
carries `<Annotation Term="Temper.Twin" String="Deep Sci-Fi"/>` under `Dsf.Twin`
and its 25 `<Annotations Target=…>` blocks with `Temper.References`, and
Foundry's Twins page lists the twin as a graph with edges. No temperpaw code changes.
