# ARN-438 - temperpaw side: shadow sweep both repos + keep the temper pin current

This is the temperpaw slice of ARN-438 (temper stage-2 completion + stage-3 shadow
extension). The full five-item effort and its rulings live in the Linear issue and
in the canonical design chain in `arni-labs/stack` (`docs/efforts/ARN-438/`). Two
of the five items ship as temperpaw CI:

1. **Shadow sweep, two repos.** The stage-3 S1 shadow sweep already runs nightly
   over this repo's PRs (ARN-431). temper is the kernel this app pins, and its PRs
   deserve the same shadow coverage. Extend the ONE `shadow-sweep.yml` workflow to
   sweep both `nerdsane/temperpaw` and `nerdsane/temper` - not a second workflow.

2. **Keep the temper pin current.** temperpaw pins the temper kernel by git rev in
   two manifests. Bumping that pin IS temper's primary deploy leg: a new kernel
   reaches production by riding a temperpaw release, not by deploying temper
   directly. Add a scheduled workflow that opens a PR bumping the pin whenever
   temper's main moves ahead of it. The PR rides temperpaw's normal gates.

## Why now

The stack half of item 1 (the sweep script made repo-aware, with repo-qualified
entity ids so the two repos' PR-number spaces cannot collide in prod Temper) has
landed on stack main. This PR wires the temperpaw CI that drives it, and adds the
pin-bump automation.

## Not in scope here

Items 2 (verify-temper feature map), 3 (delete temper's deploy-observe.yml), and 5
(aya temper-server release gating) land in their own repos/config per the effort
plan. This PR is temperpaw CI only.
