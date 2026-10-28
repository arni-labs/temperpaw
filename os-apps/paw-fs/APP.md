# paw-fs

TemperFS — the platform file system. Manages workspace lifecycle, directories, and files with versioning and locking.

## Entity Types

### Workspace
Top-level lifecycle/config container. Agent PawFS tools do not use Workspace as a path-operation actor; they read and write `Directory`/`File` metadata directly and stream content through `Files('{id}')/$value`.

- **States**: Active <-> Frozen -> Archived
- **Key actions**: `Create` (name, quota_limit), `UpdateQuota`, `IncrementUsage`, `DecrementUsage`, `IncrementFileCount`, `DecrementFileCount`, `Freeze`, `Thaw`, `Archive`
- **Legacy counters**: `used_bytes` and `file_count` are non-authoritative until quota/accounting is redesigned.
- **Legacy filesystem actions**: `MkDir`, `CreateFile`, `ResolvePath`, `ListDir`, `DeleteFile`, and `Rename` remain only for FUSE compatibility.

### Directory
Hierarchical container within a workspace.

- **States**: Active -> Archived
- **Key actions**: `Create` (name, path, workspace_id), `AddChild`, `RemoveChild`, `Rename`, `Archive`
- **Invariant**: Can only archive when empty (`item_count <= 1`)

### File
Single file with content lifecycle. Content stored externally; `StreamUpdated` fires after upload succeeds.

- **States**: Created -> Ready <-> Locked -> Archived
- **Key actions**: `Create` (name, path, directory_id, workspace_id), `StreamUpdated` (content_hash, size_bytes, mime_type, version_number, previous_version_id, created_by), `Lock`, `Unlock`, `Archive`
- **Invariant**: Files in Ready or Locked state always have content

### FileVersion
Immutable record of a specific file version. Each upload spawns a fresh `FileVersion` entity and links it back to the previous immutable version before superseding it.

- **States**: Current -> Superseded
- **Key actions**: `Create` (file_id, version_number, content_hash, mime_type, size_bytes, previous_version_id, created_by), `Supersede`
- **Deletes**: a version belongs to its File, so deleting a File deletes its versions (`Relation.DeletePolicy` cascade on `FileVersion.File`). This needs a Temper kernel that reads that annotation; on an older kernel the File delete is refused while it has versions.

## Setup

No dependencies. This is a foundational app — other apps (paw-agent, paw-foresight) depend on it for file storage.
