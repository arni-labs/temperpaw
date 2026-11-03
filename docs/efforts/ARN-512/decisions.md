# ARN-512 decisions (temperpaw side)

## D1 — pin, do not re-vendor

**Decision:** the only change is the kernel rev (both crates that pin it) and
the matching lockfile entries.

**Came up because:** the kernel fix has to reach the openpaw image.

**Options:** wait for the daily pin-bump bot (its PR carries no design chain
and fails the gates); bump by hand under this effort (chosen).

**Where:** `crates/temperpaw/Cargo.toml`, `crates/paw-codex-worker/Cargo.toml`, `Cargo.lock`.

## D2 — second pin, for the edges

**Decision:** pin again at nerdsane/temper#472 (`bd15e892`): targeted `<Annotations>` blocks (the twin's reference edges) survive the kernel.

**Came up because:** with #470 live the page listed the twin as a ring of unrelated nodes.

**Options:** wait for the daily pin-bump bot (rejected: no design chain, fails the gates); wait and batch with a later kernel change (rejected: the page stays wrong meanwhile); pin by hand now (chosen).

**Chose pinning now over waiting because:** the edges are the visible half of the fix and the pin is a three-file change; given up: one more image build and deploy today.

**Where:** same three files.

## D3 — a Datadog monitor's role is `monitor`, not `resource` (Rita, 2026-09-15)

**Decision:** the twin generator gives `DsfDatadogMonitor` `Temper.Role = "monitor"` (provider stays `datadog`); every other provider-managed type stays `resource`.

**Came up because:** the Twins page coloured monitors as resources; a monitor watches the app rather than serving it.

**Options:** leave it and colour by provider in the page (rejected: the role is the twin's concern, the page should not second-guess it); a `monitor` role (chosen).

**Where:** `os-apps/dsf-twin/specs/generate.py` (`PROVIDER_ROLES`), regenerated `model.csdl.xml`, `test_names.py`.
