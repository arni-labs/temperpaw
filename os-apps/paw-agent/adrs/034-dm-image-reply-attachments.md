# ADR-034: Session Reply Attachments for Generated Media

- Status: Accepted
- Date: 2026-06-17

## Context

`temper.image_generate(...)` now creates a `MediaGenerationRequest` and stores the generated image in PawFS, but terminal Session replies previously carried only text. That meant a generated image could be durable in PawFS while the final Discord DM had no machine-readable file handle to deliver.

The model may mention an image path in prose, but prose is not an attachment contract. The Session state machine needs to show, by state transition alone, which generated artifacts should be attached to a reply.

## Decision

`Session` has a string state field named `reply_attachments_json`. Monty extracts PawFS file metadata from image tool results and writes a compact JSON array into that field when dispatching `HandleToolResults` or `RecordResult`.

The attachment entries use this shape:

```json
{
  "kind": "pawfs_file",
  "file_id": "fl-...",
  "file_version_id": "fv-...",
  "filename": "image.png",
  "mime_type": "image/png",
  "path": "/images/image.png",
  "media_generation_id": "en-..."
}
```

`provider_response_applier`, steering terminal actions, and `agent_reply` preserve this field through terminal completion. `agent_reply` forwards it to `Channel.SendReply`.

## Consequences

Generated media delivery is visible in entity state and does not depend on hidden Rust orchestration or LLM prose. Channel transports can deliver attachments by reading `reply_attachments_json` without knowing how the image was generated.

The field is a JSON string rather than a native list because current IOA/CSDL action params already carry strings consistently across the Session and Channel apps.
