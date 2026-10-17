# TemperFS (workspaces and files)

## Sub-features
Workspace, Directory, File, FileVersion, ArtifactBatch, usage buckets.

## How to get to it (user POV)
Agents get versioned workspaces; files carry versions; artifact batches apply grouped writes.

## Driving it
Create a Workspace, create a File entity directly (the hot path per ADR-001 - NOT Workspace.CreateFile, which is deprecated FUSE-only), write a FileVersion via StreamUpdated, apply an ArtifactBatch; read each back. `scripts/production_artifact_batch_e2e.sh` drives the batch path verbatim.

## What proves it
Proof: StreamUpdated moves the File to Ready, LastVersionId set, the FileVersion in Current and the prior one Superseded. File.RecordVersion is internal (file-service principal), not operator-callable - do not drive it directly. Batch apply produces every member File plus a WorkspaceUsageBucket with the summed BytesDelta.

## Gotchas
The three WASM modules (workspace_fs, blob_adapter, artifact_batch_apply) are criticality=app-required and live one directory deeper than other apps' build scripts; `make wasm` covers both depths. The boot-kill on missing modules only fires on a fresh install of a CHANGED bundle (a Skipped unchanged bundle bypasses the gate). No 32KB truncation - the inline ceiling is 128KB with transparent blob overflow (ADR-0033).
