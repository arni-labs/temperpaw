# Agent lifecycle (core)

## Sub-features
Agent, Session, SessionEntry/Link, Soul, Team, Memory, Plan, CronJob, CronScheduler, App, ToolHook, provider-auth entities. (Project has a spec but no OData entity set - unreachable; do not drive it.)

## How to get to it (user POV)
Everything else runs through this: agents exist, hold sessions, record memory, schedule cron work.

## Driving it
Create an Agent (body {}), dispatch TemperPaw.Configure with name/role/model/provider params, read back Status=Active (verified live). Session chain reads via filter+paging (SessionEntries?$filter=SessionId eq '..'&$top&$skip) - there is NO $expand (no NavigationProperty in the CSDL). SessionEntry has no actions and a composite key - its fields ride the create body.

## What proves it
The Agent moved Created -> Active with name persisted. Session graph reads by SessionId filter, not $expand.

## Gotchas
State vars set via action params for entities that HAVE actions (Agent.Configure); entities like SessionEntry carry fields in the create body instead. Persisting via actions is the rule; a raw field PATCH has no route (404/405), not a Cedar 403.
