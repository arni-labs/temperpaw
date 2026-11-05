# Contributing

## Quick start

1. Run `make setup`.
2. Start the daemon with `make dev`.
3. Open `http://localhost:3467/dashboard`.

## Local Temper development

TemperPaw defaults to the upstream Temper git dependency. If you need to work against a sibling Temper checkout, copy `.cargo/config.toml.example` to `.cargo/config.toml` and uncomment the patch entries you need.

## TDD expectations

TemperPaw follows red-green-refactor for code changes:

1. Add the failing test first.
2. Implement the smallest change that makes it pass.
3. Refactor while keeping the suite green.

## Verification

Before opening a PR, run:

- `cargo test -p temperpaw --quiet`
- `cd dashboard && npm run build`

If you touch end-to-end flows, capture proof in `.proofs/`.

A documentation-only change lands on `main` like any other change.

A push to the default branch starts its line by itself, with no step by hand.
