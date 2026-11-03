# ADR-005: Discord Delivery of PawFS Reply Attachments

- Status: Accepted
- Date: 2026-06-17

## Context

`Channel.SendReply` previously delivered text, embeds, and components only. Discord file uploads require a multipart request with `payload_json` plus `files[n]` parts, so a PawFS-backed image could not be sent back to a DM even when generation succeeded.

## Decision

`Channel.SendReply` and `Channel.ReplyDelivered` include an optional `reply_attachments_json` parameter. The `send_reply` WASM forwards that value to the transport webhook.

The Discord transport parses PawFS attachment metadata, downloads bytes from `/tdata/Files('<file_id>')/$value` with the existing internal Paw API client, and uploads them via a dedicated `send_discord_message_with_files` helper using Discord's multipart message shape.

## Consequences

Discord DMs can receive generated images as real file attachments. The Channel entity records the same attachment metadata that was handed to the transport, preserving the Temper-native audit trail.

Attachment delivery remains transport I/O. Business decisions about which artifacts to attach stay in Session state and WASM integrations.
