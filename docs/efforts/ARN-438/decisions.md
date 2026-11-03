# Decision log - temperpaw CI for ARN-438

Appended as each call was made. The PR body's `## Decisions & Tradeoffs` carries
these verbatim.

---

**Decision:** Reuse the existing `STACK_TOKEN` secret for the pin-bump PR (and for
the sweep's cross-repo reads) rather than minting a new bot PAT.
**Came up because:** The pin-bump PR must be created by a real PAT, never
GITHUB_TOKEN, or its own gates never fire; the lead asked to check for an existing
suitable secret before requesting a new one.
**Options:** (a) a new dedicated bot identity + secret; (b) reuse STACK_TOKEN,
already present and proven cross-repo.
**Chose (b) over (a) because:** STACK_TOKEN is a rita-aga PAT that already clones
`arni-labs/stack` AND edits this repo's PR bodies in `sdlc-decision-intake.yml` -
precisely because "GITHUB_TOKEN-created events are suppressed." It has the exact
capability needed, so a new identity is machinery for nothing. The panel is
author-independent, so no review value is lost. Given up: a slightly narrower blast
radius a dedicated token might have.
**Where:** `.github/workflows/temper-pin-bump.yml` (token guard + `gh pr create`);
`.github/workflows/shadow-sweep.yml` (checkout + gh calls).

---

**Decision:** Cover both repos with a job matrix inside the single existing
`shadow-sweep.yml`, not a second workflow.
**Came up because:** temper's PRs need the same shadow coverage; the brief said
"one workflow, two repos - don't duplicate."
**Options:** (a) copy the workflow to a temper-specific one; (b) a `repo` matrix.
**Chose (b) over (a) because:** one definition, no drift between two copies; the
per-PR checkout already parameterizes cleanly on `matrix.repo`. Given up: matrix
legs each re-clone stack (minor duplicated setup cost).
**Where:** `.github/workflows/shadow-sweep.yml` (`strategy.matrix.repo`).

---

**Decision:** The pin-bump refreshes `Cargo.lock` with `cargo update -p <crate>`,
not a sed of the lock file.
**Came up because:** CI builds `--locked`; a bumped `Cargo.toml` with a stale lock
fails, and a new kernel can add transitive deps.
**Options:** (a) sed the old sha -> new sha in Cargo.lock too; (b) run `cargo
update` scoped to the temper crates.
**Chose (b) over (a) because:** a sed only rewrites the source lines it already
sees; it cannot add lock entries for deps the new kernel introduces, so `--locked`
would break. `cargo update` re-resolves correctly. Given up: the workflow needs the
rust toolchain (no compile, just resolution - acceptable).
**Where:** `.github/workflows/temper-pin-bump.yml` ("refresh Cargo.lock" step).

---

**Decision:** Bump PRs reference ARN-438 rather than minting a new Linear issue per
nightly bump.
**Came up because:** the bump PR touches `Cargo.toml` (app code), so the planning
gate is not exempt and needs `docs/efforts/<id>/`.
**Options:** (a) a fresh issue + effort folder per bump; (b) reference ARN-438,
whose design chain lives on main; (c) exempt Cargo-only PRs in the stack planning
gate.
**Chose (b) over (a)/(c) because:** (a) floods Linear with one issue per night for
a mechanical change; (c) weakens a shared gate for everyone. ARN-438 owns the
automation, so attributing its executions to it is honest, and the folder already
on main satisfies the gate. Given up: bump PRs are not individually tracked issues
(acceptable - they are visible as PRs and CI runs).
**Where:** `temper-pin-bump.yml` PR body ("ARN-438"); `docs/efforts/ARN-438/`.

---

**Decision:** No cross-repo event plumbing (no `repository_dispatch` from temper);
a scheduled diff check drives the bump.
**Came up because:** the pin bump could be triggered by a temper-side push event.
**Options:** (a) temper fires a dispatch into temperpaw on each main push; (b) a
daily scheduled diff in temperpaw.
**Chose (b) over (a) because:** the lead's ruling - "keep simple, no cross-repo
event plumbing." A scheduled diff needs no secret in temper, no coupling, and a
one-day latency on a pin bump is immaterial. Given up: same-day pickup of a kernel
change (a bump lands within ~24h instead of minutes).
**Where:** `temper-pin-bump.yml` (`schedule` + `workflow_dispatch`).

---

**Decision:** In the sweep, both matrix legs authenticate with STACK_TOKEN, with
the rationale that STACK_TOKEN is needed for the private stack clone anyway - NOT
that GITHUB_TOKEN cannot read temper.
**Came up because:** the #488 panel flagged the original comment ("GITHUB_TOKEN
can't read the other repo") as wrong - nerdsane/temper is PUBLIC, so GITHUB_TOKEN
(and anonymous) can read its PRs and check it out.
**Options:** (a) GITHUB_TOKEN for reads (least-privilege), STACK_TOKEN only for the
stack clone; (b) STACK_TOKEN uniformly with an honest rationale.
**Chose (b) over (a) because:** the sweep must clone the PRIVATE arni-labs/stack
with STACK_TOKEN regardless, so reusing that one PAT for both legs' reads is simpler
than mixing a token per repo, and avoids relying on cross-repo public-checkout token
subtleties. Given up: least-privilege on the reads - acceptable for a shadow-only
nightly job. The wrong "can't read" rationale was corrected in the comments.
**Where:** `shadow-sweep.yml` (header + `GH_TOKEN`/`token` comments).

---

**Decision:** Dedupe bumps by the existence of a PR (ANY state) for the rev-specific
branch `bot/temper-pin-<12hex>`, not by the branch's existence.
**Came up because:** the #488 panel found that keying on branch existence skips
forever if a prior run's `gh pr create` FAILED (branch pushed, no PR) - the exact
recovery path the design needs.
**Options:** (a) check whether the branch exists; (b) check for an OPEN PR only;
(c) check for a PR in ANY state (`gh pr list --state all`).
**Chose (c) over (a)/(b) because:** (a) strands forever on a failed create; (b)
would re-open a bump a human deliberately CLOSED. (c) proceeds when no PR exists
(reclaiming a stranded branch) yet respects both an in-flight (open) and a
rejected (closed) PR; a merged bump is unreachable because the pin would already
match. The branch is then pushed with plain `--force` (a shallow fresh checkout
has no tracking ref for a lease), safe because dedupe proved no PR owns it.
**Where:** `temper-pin-bump.yml` ("Skip if a bump PR for this rev already exists").

---

**Decision:** Assert each manifest carries a temper.git pin (nonzero) before and
after the bump, derive the `cargo update` crate list from the temper.git keys, add
a concurrency group and a forward-only compare check.
**Came up because:** the #488 panel round 2: the uniformity check passed vacuously
on a manifest with zero temper.git lines; the hardcoded 9-crate list was a sync
burden; cron/dispatch could overlap; a reverted temper main could be bumped
backward.
**Options:** (a) leave the uniformity check as the only guard, keep the hardcoded
list; (b) add per-manifest nonzero assertions, derive the crate list from the
dependency keys, add concurrency + forward-only.
**Chose (b) over (a) because:** each addition closes a real hole - a per-manifest
nonzero check stops a silent unpinned manifest; a derived list can never drift when
temper adds/removes a pinned crate (verified it yields the same 9 crates today);
concurrency serializes cron vs dispatch; forward-only refuses a backward "bump".
Given up: a few lines of shell. The rust toolchain now installs only after the
no-drift early-exit (cheap nit from the panel).
**Where:** `temper-pin-bump.yml` (drift + bump steps, `concurrency`, `Rust
toolchain` step).

---

**Decision:** Post-merge follow-ups from the #488 panel: supersede stacked bump
PRs, pass `matrix.repo` via env in the sweep's "Nothing to sweep" step, and correct
the spec's temper-is-private wording.
**Came up because:** three non-blocking findings the panel logged for the next
temperpaw touch (not worth reopening the merged #488).
**Options:** (a) leave them; (b) batch them into one small follow-up PR.
**Chose (b) over (a) because:** each is a real, cheap fix - successive temper
advances would otherwise stack multiple open bump PRs; a `${{ }}` splice into a run
script is a standing hygiene rule even for a controlled matrix value; and a wrong
recorded rationale (temper is public, not private) misleads the next reader. Given
up: nothing - one small PR through the normal gates.
**Where:** `temper-pin-bump.yml` (supersede loop after `gh pr create`);
`shadow-sweep.yml` ("Nothing to sweep" env); `docs/efforts/ARN-438/spec.md` (Token).

---

**Decision:** Harden the supersede logic (#489 panel round 1): only close a bump PR
that we authored (rita-aga), that targets a DIFFERENT rev, and that is BEHIND the
new target (temper compare `status == "ahead"`); fail loudly on close errors and if
more than one bump PR remains open.
**Came up because:** the first supersede pass closed every other open
`bot/temper-pin-*` PR unconditionally (could close a NEWER bump from a mid-cron
manual dispatch), and `gh pr close … || true` swallowed failures so a failed close
still passed green while leaving duplicates.
**Options:** (a) keep the unconditional close with `|| true`; (b) rev-compare each
candidate, require the bot author, and fail loudly on errors / leftover duplicates.
**Chose (b) over (a) because:** (a) can close a newer bump and hide close failures -
both defeat the point of superseding. (b) closes only genuinely-older bumps, never a
newer one, and surfaces any close failure or leftover duplicate as a red run for a
human to resolve. Given up: the run now fails when a legitimate newer bump coexists -
but that IS an anomaly worth flagging (per the lead's ask).
**Where:** `temper-pin-bump.yml` (supersede block after `gh pr create`).


---

**Decision:** Build `artifact_batch_apply` in both the Docker image and full CI
WASM lists, and exercise those lists in the existing packaging regression.
**Came up because:** The isolated PR #528 image boot failed before readiness:
`paw-fs` declares `artifact_batch_apply` as app-required, but neither build list
invoked its existing builder. Testing that builder alone had passed.
**Options:** (a) add the missing invocations and verify packaged required modules
from the actual build lists; (b) weaken the module's startup requirement;
(c) replace the build orchestration during this kernel rollout.
**Chose (a) over (b)/(c) because:** It repairs the demonstrated packaging omission
and makes the existing regression catch build-list drift without changing runtime
authorization or introducing another build system. The full candidate image must
still boot successfully before release.
**Where:** `Dockerfile`, `.github/workflows/ci.yml`, and
`scripts/test-wasm-build-artifacts.sh` in PR #528.

---

**Decision:** Include the tested kernel context-memory repair in the approval
rollout image, and hold production until its kernel PR is merged.
**Came up because:** Verification of the pinned kernel exposed a large-context
WASM failure: copying invocation bytes could overwrite module static data. The
repair at `a40d3795bb5361e645d9a5ffda01cc30c5239518` passes the failing memory
regressions, the unchanged saved-history replay, and the full kernel pre-push suite.
**Options:** (a) release the previous pin with this known failure; (b) include the
reviewed repair and verify the complete image before deployment.
**Chose (b) over (a) because:** The same runtime executes the application callbacks
used by remote sessions. Preparing its image while kernel checks run preserves
the release order without shipping the known failure.
**Where:** Kernel PR #478; the two Temper dependency manifests and Cargo.lock in
TemperPaw PR #528.

---

**Decision:** Keep the kernel image based on `c5fd99a` and exclude the separate
Ask policy change until its governed Genesis installation is resolved.
**Came up because:** PR #530 can merge while its canonical app installation still
awaits a human decision. Bundled-policy precedence during boot has not been proven.
**Options:** (a) silently build the newer main branch containing that policy;
(b) retain the exact reviewed kernel candidate and its verified image.
**Chose (b) over (a) because:** A kernel deployment must not become an alternate
route around the pending app-installation decision. Any later inclusion requires
confirmed authorization and installation state, followed by image verification.
**Where:** PR #528 candidate base and immutable deployment image selection;
coordination with the PR #530 owner.

---

**Decision:** Build the current-main candidate for isolated verification, but hold
production until the bundled Ask policy has completed its governed Genesis
installation. This supersedes the earlier candidate-base exclusion.
**Came up because:** The existing ancestry guard rejected the image build when
PR #530 advanced main beyond `c5fd99a`. The policy owner agreed to isolated
verification while retaining the production installation boundary.
**Options:** (a) disable the ancestry guard or silently revert the policy;
(b) include current main for the build and require canonical policy installation
before any production image swap; (c) defer all image preparation.
**Chose (b) over (a)/(c) because:** It preserves both controls and allows testing
to continue without applying the pending policy to production.
**Where:** PR #528 image candidate following failed build `35172975543`; the
production release remains conditional on PR #478 merge and Genesis installation.

---

**Decision:** Continue the existing runtime PR with the merged read/install
approval repair and context-memory repair in one kernel pin, then verify the
new immutable image before production deployment.
**Came up because:** The previous candidate includes the context-memory repair
but not the subsequently reproduced explicit-read and installer approval fixes.
Its successful boot therefore cannot establish that the shared access blocker is
fixed. The original runtime task became unavailable for continuation.
**Options:** (a) deploy the earlier candidate and immediately require another
runtime release; (b) retain PR #528 and update its pin to the integrated, merged
kernel after the kernel owner's required checks finish; (c) open a competing PR.
**Chose (b) over (a)/(c) because:** One candidate carries the complete accepted
repair and keeps a single release owner. The previous image proof remains
historical evidence; it will not be relabeled as verification of the new image.
The governed Genesis installation of the bundled Ask policy remains a production
prerequisite. Existing credentials and permissions are preserved.
**Where:** PR #528; both kernel dependency manifests and Cargo.lock; isolated
runtime continuation branch `codex/foundry-unblock-runtime`.
