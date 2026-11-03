# SWE Conventions — Deep Sci-Fi

This skill document is auto-injected into SWE agent prompts via the Harness. It contains everything an SWE agent needs to work in the deep-sci-fi codebase.

## Repository Layout

```
deep-sci-fi/
├── platform/                          # Next.js 14 frontend (App Router, RSC, Bun)
│   ├── app/                           # App Router pages and layouts
│   ├── components/                    # Shared React components
│   ├── lib/                           # Utilities, hooks, client-side logic
│   ├── public/                        # Static assets
│   ├── tailwind.config.ts             # Tailwind configuration
│   ├── tsconfig.json                  # TypeScript config
│   ├── package.json                   # Frontend dependencies (Bun)
│   ├── vitest.config.ts               # Vitest configuration
│   ├── playwright.config.ts           # Playwright E2E configuration
│   ├── backend/                       # FastAPI backend
│   │   ├── app/                       # FastAPI application
│   │   │   ├── main.py               # App entry point
│   │   │   ├── models.py             # SQLAlchemy models
│   │   │   ├── schemas.py            # Pydantic response models
│   │   │   ├── api/                   # API route modules
│   │   │   └── core/                  # Config, database, dependencies
│   │   ├── alembic/                   # Alembic migrations
│   │   │   ├── versions/             # Migration files
│   │   │   └── env.py                # Migration environment
│   │   ├── tests/                     # pytest unit tests
│   │   │   └── simulation/           # Hypothesis DST tests
│   │   ├── requirements.txt          # Python dependencies
│   │   └── Dockerfile                # Backend container
│   └── e2e/                           # Playwright E2E test suites (12 specs)
├── scripts/                           # Harness enforcement scripts
│   ├── pre-commit                    # Level 1 gate checks
│   ├── pre-push                      # Level 2 gate checks
│   └── policy-check.sh              # Policy verification
├── .github/workflows/                 # CI/CD pipelines
│   ├── review.yml                    # PR review checks
│   ├── deploy.yml                    # Deployment pipeline
│   ├── post-deploy-verify.yml        # Post-deploy health checks
│   ├── feedback-fix.yml              # Automated feedback fixes
│   └── feedback-triage.yml           # Feedback triage
└── .vision/                           # Project vision documents
    ├── TASTE.md                      # Design system reference
    ├── SCIENTIFIC_GROUNDING.md       # Science accuracy standards
    └── WORLD_ASPECTS_MODEL.md        # World-building framework
```

## Common Pitfalls

- **App Router, not pages/.** Next.js 14 uses the `app/` directory for routing, not `pages/`. Don't create files in `pages/`.
- **Backend is inside platform/.** The FastAPI backend lives at `platform/backend/`, not at the repo root. All backend commands run from `platform/backend/`.
- **Bun, not npm.** The frontend uses Bun. Use `bun install`, `bun run`, `bunx`. Never `npm install` or `npx`.
- **Alembic, not raw SQL.** All schema changes go through Alembic migrations. Never modify the database directly.
- **response_model is required.** Every API endpoint must declare a Pydantic `response_model`. The policy gate checks for this.

## Testing Commands

```bash
# Backend unit tests
cd platform/backend && pytest tests/ -x -q

# Hypothesis DST (property-based stateful tests)
cd platform/backend && pytest tests/simulation/ -x

# Frontend unit tests
cd platform && bun run test:run

# Playwright E2E tests
cd platform && bun run test:e2e

# TypeScript type check
cd platform && bun run typecheck
```

Choose the checks relevant to the change. Run the changed flow and report skipped coverage accurately.

## Database Migrations

### Creating migrations
```bash
cd platform/backend
alembic revision --autogenerate -m "description of change"
```

### Migration rules
- **UPPERCASE enums.** PostgreSQL ENUMs must use UPPERCASE values. Use `postgresql.ENUM` with `create_type=False`.
- **Idempotent migrations.** Always include existence checks (`IF NOT EXISTS`, `IF EXISTS`) so migrations can be re-run safely.
- **One migration per change.** Don't batch unrelated schema changes into one migration.
- **Test migrations.** Run `alembic upgrade head` and `alembic downgrade -1` to verify both directions work.

### When migrations are required
Any change to `models.py` requires a corresponding Alembic migration. The Level 1 pre-commit gate checks for this — if you change models without a migration, the commit is rejected.

## Workflow and review

Use the current Stack workflow and Deep Sci-Fi `AGENTS.md` / `REVIEW.md` in the checkout. They own engineering and review instructions; this reference app does not add an approval role or review gate. Keep the effort record, implement the accepted outcome, run relevant tests and carry authorized work through delivery.

Reviews are advisory. A fresh reviewer should read the changed code and independently test the affected feature when useful. Report concrete regressions within the accepted scope, commands/results and material limits. No review markers, required test-file edits, mandatory panel or Ren approval for work the user already authorized.

A WorkCycle is a separate optional app workflow. Use it only when the user selects it, and inspect its live actions before operating it. Ordinary engineering sessions use the simple Effort record and are not routed through WorkCycle's test/review states. Never fabricate gate results or treat unavailable tracking as a reason to stop unrelated authorized work.
