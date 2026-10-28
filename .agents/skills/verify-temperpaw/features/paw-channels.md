# Channels (Discord/Slack/webhook messaging)

## Sub-features
ChannelSession, Channel, AgentRoute, TransportConnection - inbound messages route to agent sessions; Discord DMs are the human channel.

## How to get to it (user POV)
A person messages the bot on Discord; the platform routes to an agent and replies in-channel.

## Driving it
Requires paw-agent installed (routing calls /tdata/Agents,/Sessions). Register an AgentRoute with channel_id, Configure a Channel with channel_type=cli (inline delivery, no webhook needed), Connect it (expect Connected synchronously - channel_connect emits Ready), then dispatch Paw.Channel.ReceiveMessage. Read ChannelSessions and the produced Session back.

## What proves it
Pass: a ChannelSession in Active with channel_id/thread_id/agent_entity_id/session_entity_id populated (its only other state is Expired - not a traversal). Route resolution is exact channel_id then wildcard only - the documented binding-tier priority is NOT implemented, so compare the resulting agent_config to prove the right route matched.

## Gotchas
TransportConnection only supervises the live Discord gateway (transport_reconcile fires on Start/RetryDue) and is NOT in the OData routing path - skip it unless testing live Discord. Namespace is Paw.Channel.<Action>. A non-cli/tui channel with empty webhook_url ReplyFails on SendReply.
