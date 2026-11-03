# ADR-004: Seeded Discord DM Catch-Up

## Status

Accepted

## Context

The Discord transport stores a cursor for the latest processed Discord message and attempts to replay missed DM messages after gateway reconnect. The replay path used Discord's current-user DM channel listing as the source of channels to scan.

In production, the bot token can open the known DM with `POST /users/@me/channels`, but `GET /users/@me/channels` returns an empty list. That made reconnect catch-up a no-op even when the Channel entity still knew the last DM thread/user.

## Decision

When reusing a Discord Channel entity, the transport seeds its DM-channel cache from persisted `thread_id` or `author_id` by reopening that DM through Discord's create-DM endpoint. Catch-up now merges any REST-listed DMs with those seeded mappings and fetches messages newer than the persisted cursor from both sources.

## Consequences

Gateway reconnect can replay missed DMs for known conversations even when Discord does not list DM channels for the bot. Reply delivery continues to use the same cache, and live gateway messages still update it naturally.
