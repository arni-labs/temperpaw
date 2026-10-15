# Feature map

Surface enumeration (os-apps/ has 18 apps; the startup surface bootstraps: paw-fs, paw-agent, paw-channels, paw-foresight, paw-media, paw-ingest, paw-pm, paw-compute, paw-patrol, paw-research, paw-skills):

| Feature | File | Drive when you changed |
|---|---|---|
| Boot + health | boot-and-health.md | server startup, wasm bundling, stores |
| OData surface | odata-surface.md | routes, auth, Cedar, any entity spec |
| Genesis install | genesis-install.md | app publish/install, registry pins |
| Agent lifecycle | paw-agent.md | paw-agent specs/wasm, sessions, cron |
| Channels | paw-channels.md | paw-channels, transports, routing |
| Patrol | paw-patrol.md | paw-patrol chain, workers, findings |
| TemperFS | paw-fs.md | paw-fs, blob/file wasm modules |
| Media | paw-media.md | paw-media generation flows |
| Research | paw-research.md | paw-research, web_fetch/web_search |
| Foresight | paw-foresight.md | paw-foresight engine entities |

## Not yet mapped

- paw-skills - install flow partly covered by genesis-install.md; native skill-package install deserves its own file
- paw-ingest - webhook ingress; drive needs signed webhook fixtures
- paw-pm, paw-compute - bootstrapped but thin surfaces today; map when driven
- paw-ai, paw-autoreason, paw-consilium, paw-harness, paw-heal, paw-managed-agents, paw-wiki - not in the startup surface; map when enabled
