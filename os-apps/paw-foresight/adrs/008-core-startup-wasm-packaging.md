# ADR-008: Core Startup WASM Packaging

## Status

Accepted

## Date

2026-06-17

## Context

`paw-foresight` is a core startup app. Its app manifest declares required corridor WASM modules, but the app did not provide a root `wasm/build.sh`, and production Docker/CI did not build its module-local artifacts. A fresh local startup with core app reconciliation failed before readiness because the required corridor modules were absent.

Core startup apps must be deployable from a clean checkout and a clean container image. Required module declarations are not enough; the app also needs a reproducible build path that leaves `.wasm` files outside pruned `target/` directories.

## Decision

`paw-foresight` has an app-level `os-apps/paw-foresight/wasm/build.sh` that builds every required corridor module and publishes each module-local `.wasm` artifact.

The production Dockerfile and CI os-app WASM build matrix include the `paw-foresight` build script. The TemperPaw identity contract audits the script alongside other startup app build scripts.

## Consequences

Fresh production images and fresh local boots can reconcile `paw-foresight` without relying on stale persisted module bundles. Corridor work remains isolated in the `paw-foresight` app, but it no longer blocks unrelated core features by being undeployable from source.
