# TemperPaw

An agent that evolves its own capabilities.

TemperPaw is an ambient agent platform. It runs in the background, always on. You talk to Paw wherever you already are — messaging, voice, text. Paw manages work, spawns specialist agents, and gives them their own computers. When an agent needs a capability that doesn't exist, it designs one. The platform verifies it. You approve it. And now every agent on the platform can use it — perpetually.

Paw runs on the same platform it gives to its agents. Its memory, skills, permissions, and workflows are built the same way agents build new capabilities. So when agents extend the platform, they're working with the same building blocks Paw itself is made of. Like a building where the tenants can add rooms — using the same materials the building is made of. The platform grows with them.

## The Human Experience

1. You deploy TemperPaw — one binary, one service, a set of environment variables.
2. You connect Paw to where you already communicate — Discord, Slack, or whatever comes next.
3. You say: **"Manage this for me."**
4. Paw takes it from there:
   - Hires specialist agents for the work
   - Gives each agent its own governed computer — scoped credentials, scoped access, isolated from everything else
   - Sets up the workflow rules for the project (and asks you to approve them)
   - Reports back to you
5. From then on, the system runs:
   - Agents handle day-to-day execution
   - Paw proactively reports progress, surfaces decisions, flags problems
   - You review outcomes and approve what matters
6. You control how much autonomy each agent has, per project, and adjust it as trust builds.

## What This Looks Like

**"Take over this codebase."** You hand Paw a repo. Agents map the architecture, set up monitoring, and establish workflow rules based on the existing conventions. Within a day, the project has active maintenance — dependency updates tested in isolation, alerts triaged, PRs opened when something breaks. You review and approve.

**"Track production for this campaign."** Dozens of deliverables across copy, design, video, and web. Agents track where every asset is in the pipeline — draft, review, revision, approved. When a reviewer is late, the agent flags it and suggests reassignment. When a dependency between deliverables shifts, you know before it blocks the launch.

**"Manage our vendor contracts."** Agents track contract terms, flag renewals 90 days out, and surface clauses that conflict with new policies. When a contract comes up for renewal, you get a summary of what changed since last time — pricing, SLAs, compliance requirements. The agent already checked.

**An agent keeps running into the same problem.** Across different projects, it keeps hitting the same friction — a pattern it has to work around every time. So it abstracts the pattern into a reusable capability, the platform verifies it, and you approve it. Now every agent on the platform can use it. The system grew a new organ.

**You go to sleep.** While you're gone, an evolution agent watches how the other agents work. It notices friction — a workflow that takes too many steps, a tool that keeps failing. It designs an improvement, tests it, and queues it for your approval. You wake up to a platform that's better than the one you left.

## Core Concepts

### Agents as Humans

Every agent is modeled as a human equivalent. When an agent gets "hired," it's like a new team member getting a laptop, credentials, and a project assignment.

- **Paw** — The manager. Talks to you, delegates work, reports progress. Coordinates across projects.
- **Specialists** — Domain experts assigned to specific work. A software engineer maintains a codebase. An SRE triages alerts. A production coordinator manages schedules. Each gets their own computer and scoped access.
- **Evolution Agents** — The platform engineers. Watch how other agents use their tools, identify friction, and improve the tools and workflows agents use.

### The Harness (Project-Specific Rules)

Every project has a harness — the rules that govern how work happens on that project.

The agent assigned to a project creates the harness, informed by the project's existing conventions. You approve it. From then on, the platform enforces it structurally — agents follow the required steps because the system requires them, at every transition.

Different projects have different harnesses. A software project has deployment rules and testing requirements. A creative project has review stages and approval gates. Each harness reflects the work it governs.

### The Autonomy Slider

Per-agent, per-project, as granular as it gets.

- "This agent can schedule tasks freely, but must ask before committing budget"
- "This agent can open pull requests, but must ask before merging"
- "This agent can adjust timelines, but must notify me when deadlines move"

You decide where the line is. As trust builds, you move it.

### Computer Governance

Every agent's computer is governed:

- **Scoped credentials** — only the access needed for its assigned project
- **Network boundaries** — controlled access to external services
- **Isolation** — Agent A's environment cannot reach Agent B's projects or secrets

### The Evolution Loop

Dedicated agents that run continuously in the background:

1. Watches agent activity across all projects — tool usage, workarounds, failures, friction
2. Identifies unmet needs — when agents work around something that should be direct
3. Designs and tests improvements to tools and workflows
4. Surfaces proposals for your approval

The platform gets better over time, and you don't have to design every improvement yourself.

### Agent-Created Capabilities

Agents create capabilities as tools for themselves — structured workflows that make their work easier. A specialist agent creates a tracking system for its project. A team of agents creates a shared coordination tool with custom stages. These agent-created capabilities are what the evolution agents watch and improve.

## What Paw Can Do Today

- **Talk to you on Discord and Slack** — holds conversations, takes direction, reports back
- **Spawn agents with their own computers** — isolated sandboxes with scoped credentials
- **Manage projects** — track issues, create plans, coordinate work
- **Research the web** — search, fetch, and synthesize information
- **Read and write governed files** — every file operation is authorized
- **Remember across sessions** — persistent memory, skills, and identity

## Where We're Going

**Self-extending capabilities.** Agents design, verify, and install new capabilities at runtime. The platform grows as agents work, through governed evolution.

**Structural governance.** Every capability has enforced rules — what it can access, who can use it, what transitions are valid. The platform enforces them structurally. You control how much autonomy each agent has, and adjust it as trust builds.

**The evolution loop.** Dedicated agents that watch how other agents work, identify friction, and propose improvements to tools and workflows. You review and approve. The platform gets better without you designing every improvement.

**Cross-domain agent teams.** Agents with different expertise, different permissions, different tools — collaborating through governed shared state. Engineering agents, operations agents, creative agents, all coordinating on shared work.

## Get Started

### Run locally

```bash
git clone https://github.com/nerdsane/temperpaw.git
cd temperpaw
cargo build --release
./target/release/temperpaw run
```

The CLI walks you through everything — API key, messaging, personalizing your Paw — then boots the server.

### Deploy to the cloud

```bash
cargo build -p temperpaw-cli --release
./target/release/temperpaw deploy
```

Provisions Turso, Cloudflare R2, and Railway interactively — no `.env` file needed. The deploy flow prompts for all credentials.

If you enable Datadog during deploy, traces, metrics, and logs flow to Datadog through a `datadog-runtime-agent` service:

```bash
export DD_API_KEY=your-datadog-api-key
./target/release/temperpaw deploy
```

If you don't have a Datadog key yet, deploy without it: telemetry export stays off (`OTEL_ENABLED=false`) and nothing else changes. Rerun `temperpaw deploy` with a key later to turn it on.

### Diagnose

```bash
./target/release/temperpaw doctor
```

## How It's Built

TemperPaw runs on [Temper](https://github.com/nerdsane/temper), an open-source kernel with building blocks that generalize to build software applications of any size. Temper provides verified state machines, authorization policies, and an auditable runtime. TemperPaw is one application built on Temper — and the capabilities agents create are also applications built on Temper. Same kernel, same building blocks, all the way down.

For architecture and internals, see [AGENTS.md](AGENTS.md), [docs/development.md](docs/development.md), and [docs/deployment.md](docs/deployment.md).
