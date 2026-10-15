# App install from Genesis

## Sub-features
`POST /paw/apps/install-from-genesis`, pinned refs (`owner/app@hash`), Cedar activation for the installed app's entities.

## How to get to it (user POV)
An operator installs or updates a platform app from the Genesis registry.

## Driving it
POST the install endpoint with the app ref; then list the installed apps and read one of the new app's entity sets.

## What proves it
The installed pinned ref matches what was published, and the app's entity set answers (not 403 - see the known activation gap, ARN-164). Production shape: verify on the deployed shelf per the Definition of Done, not just locally.

## Gotchas
GENESIS_TOKEN (not GENESIS_API_KEY). Install does not GC superseded Cedar (ARN-399) - policy tightening needs a check that old permits are gone.
