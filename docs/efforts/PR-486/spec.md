# Kernel pin contract

Pin every nerdsane/temper dependency in crates/temperpaw/Cargo.toml and crates/paw-codex-worker/Cargo.toml to the reviewed replacement implementation. Refresh Cargo.lock without unrelated dependency upgrades. Preserve all app pins and settings.

The new endpoint conditionally replaces one enabled Cedar entry after backend manage_policies authorization. Foundry retains private approver credentials and requires the signed-in human answer. Existing contributor/finalizer boundaries remain.

Deploy state: reviewed source -> locked daemon build -> immutable image -> governed deployment -> live source/health and changed-route verification. A failed build or mismatch is not deployed success.

The repository proof gate queries ProofPackets by the exact reviewed commit. It must find a valid current-head packet even when100 unrelated packets precede it, while retaining all Recorded/status/content validation.
