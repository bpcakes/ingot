# Package Ingot as an Electron desktop app

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. This document follows `.agent/PLANS.md` in this repository.

## Purpose / Big Picture

The repository currently has two standalone pieces: a Rust daemon named `ingotd` that exposes the HTTP and WebSocket API on `127.0.0.1:4190`, and a Vite React UI that talks to that API through the Vite development proxy. After this change a user can launch an Electron desktop app that starts or reuses the local daemon, opens the existing React UI in a native desktop window, and packages the release daemon binary with the app.

The observable behavior is that `cd ui && bun run electron:dev` opens the desktop window against the Vite dev server while ensuring the daemon is healthy, and `cd ui && bun run electron:build` creates an unpacked Electron app after building the UI and release daemon.

## Progress

- [x] (2026-05-08 00:00Z) Inspected the existing daemon, UI package scripts, Vite proxy, and renderer API assumptions.
- [x] (2026-05-08 00:00Z) Chose a desktop architecture: Electron main process supervises `ingotd`, preload exposes the API origin, and the renderer keeps using the existing API client through a URL helper.
- [x] (2026-05-08 00:00Z) Added Electron source files, package scripts, and build metadata.
- [x] (2026-05-08 00:00Z) Updated renderer HTTP and WebSocket URL construction to work from both Vite and Electron.
- [x] (2026-05-08 00:00Z) Installed and locked Electron dependencies.
- [x] (2026-05-08 00:00Z) Validated with UI tests, Electron syntax checks, UI lint, UI build, Rust release daemon build, and the Electron build script.

## Surprises & Discoveries

- Observation: The UI hard-codes same-origin `/api` and constructs WebSockets from `location.host`.
  Evidence: `ui/src/api/client.ts` defines `const BASE = '/api'`; `ui/src/stores/connection.ts` opens `${protocol}//${location.host}/api/ws`.
- Observation: The daemon binds a fixed local address, so the desktop shell should avoid launching a second copy if an existing standalone daemon is already healthy.
  Evidence: `apps/ingot-daemon/src/main.rs` binds `127.0.0.1:4190`.
- Observation: Electron Builder auto-discovered a local Apple signing identity and then hung inside `codesign` during unattended `electron-builder --dir`.
  Evidence: The first `cd ui && bun run electron:build` reached `signing file=release/mac-arm64/Ingot.app ...` and stayed there until terminated with SIGTERM.
- Observation: A packaged `ingot://app` renderer should not call the daemon through absolute HTTP fetches because that makes the request cross-origin from Chromium's perspective.
  Evidence: The backend has no CORS layer, and the renderer would otherwise fetch from origin `ingot://app` to `http://127.0.0.1:4190`.

## Decision Log

- Decision: Keep the Rust daemon as the backend process and let Electron supervise it rather than embedding Rust into the renderer.
  Rationale: This preserves the current crate boundaries and keeps `apps/ingot-daemon/` as wiring only.
  Date/Author: 2026-05-08 / Codex.
- Decision: In development, reuse Vite for the renderer and spawn `cargo run --bin ingotd` only when `http://127.0.0.1:4190/api/health` is not already healthy.
  Rationale: This matches the standalone workflow and avoids port conflicts when the daemon is already running.
  Date/Author: 2026-05-08 / Codex.
- Decision: In packaged mode, serve `ui/dist` through an Electron custom `ingot://app/` protocol and expose `http://127.0.0.1:4190` to the renderer through a preload bridge.
  Rationale: `file://` breaks browser routing and relative API calls, while a custom protocol gives the React app a stable origin without requiring the Rust daemon to serve static assets.
  Date/Author: 2026-05-08 / Codex.
- Decision: Disable code-signing identity auto-discovery for `electron:build` while leaving `electron:dist` as the release-oriented command.
  Rationale: The local unpacked build is a validation and development artifact, so it must run without interactive keychain prompts. Signed distributables can use `electron:dist` in a configured release environment.
  Date/Author: 2026-05-08 / Codex.
- Decision: In packaged mode, proxy HTTP API requests through the `ingot://app/api` protocol handler and connect WebSockets directly to the daemon.
  Rationale: Protocol-proxied HTTP stays same-origin for the renderer and avoids changing backend CORS behavior. WebSocket connections are already direct local connections and are not blocked by browser CORS preflight.
  Date/Author: 2026-05-08 / Codex.

## Outcomes & Retrospective

The Electron desktop app is implemented. `cd ui && bun run electron:dev` starts Vite, then launches Electron; the Electron main process starts `ingotd` only if the daemon is not already healthy. `cd ui && bun run electron:build` builds the React UI, builds the release daemon, and creates `ui/release/mac-arm64/Ingot.app` with `Contents/Resources/bin/ingotd` included as an executable extra resource.

The packaged app serves the renderer from `ingot://app/`, proxies HTTP API calls from `ingot://app/api/*` to `http://127.0.0.1:4190/api/*`, and connects WebSockets to `ws://127.0.0.1:4190/api/ws`. This avoids backend CORS changes while preserving the existing standalone daemon API.

## Context and Orientation

`apps/ingot-daemon/src/main.rs` starts the `ingotd` binary, initializes SQLite state under the default state root, runs the background dispatcher, and listens on `127.0.0.1:4190`. The React UI lives under `ui/`; `ui/vite.config.ts` starts Vite on port `4191` and proxies `/api` to the daemon. The renderer API client is in `ui/src/api/client.ts`, React Query helpers are in `ui/src/api/queries.ts`, and WebSocket connection state is in `ui/src/stores/connection.ts`.

An Electron app has a main process and a renderer process. The main process is Node.js code that creates native windows and can spawn child processes. The renderer process is the browser-like window that runs the existing React app. A preload script is a small trusted bridge that runs before the renderer and safely exposes selected values from the main process environment.

## Plan of Work

Add `ui/electron/main.cjs` for the Electron main process. It will register an `ingot://` protocol for packaged assets, create the browser window, check daemon health, launch the daemon only when needed, and shut down only the child daemon it started.

Add `ui/electron/preload.cjs` to expose `window.ingotDesktop.apiOrigin`. Add a TypeScript declaration and URL helper under `ui/src/api/` so HTTP fetches and WebSocket connections use the explicit desktop origin when present, while preserving `/api` same-origin behavior in browser/Vite tests.

Add `ui/electron/dev.cjs` so `bun run electron:dev` starts Vite, waits for it, then starts Electron with `VITE_DEV_SERVER_URL`. Update `ui/package.json` with Electron scripts and electron-builder metadata. Install `electron` and `electron-builder` as dev dependencies so `ui/bun.lock` records exact versions.

## Concrete Steps

From the repository root, edit the files listed above. Then run:

    cd ui
    bun add -d electron electron-builder
    bun run test
    bun run build
    cargo build --release --bin ingotd --manifest-path ../Cargo.toml
    bun run electron:build

The final build command should create an unpacked desktop app under `ui/release/` and include `target/release/ingotd` as an Electron extra resource.

## Validation and Acceptance

Acceptance requires all of the following, all completed on 2026-05-08:

The renderer still passes existing Vitest coverage and new URL helper coverage with `cd ui && bun run test`. The run completed with 18 test files and 72 tests passing. Existing React `act(...)` warnings appeared in `jobs-page` tests, but no assertions failed.

The UI type-checks and produces a Vite build with `cd ui && bun run build`. The build completed and emitted `ui/dist`.

The daemon release binary builds with `cargo build --release --bin ingotd --manifest-path ui/../Cargo.toml` or from the repository root with `cargo build --release --bin ingotd`. The release build completed and produced `target/release/ingotd`.

The Electron build script completes with `cd ui && bun run electron:build`, proving that the app metadata, Electron main process, UI build, release daemon binary, and extra resources configuration are consistent. The generated app bundle contains `Contents/Resources/app.asar` and executable `Contents/Resources/bin/ingotd`.

The Electron CommonJS files parse with `node --check electron/main.cjs && node --check electron/preload.cjs && node --check electron/dev.cjs`.

The UI lint gate passes with `cd ui && bun run lint`.

Manual runtime validation is `cd ui && bun run electron:dev`; it should open an Ingot window and the health query should show the daemon as connected. In environments without a GUI this may not be runnable, so build and test results are the required automated proof.

## Idempotence and Recovery

The scripts are safe to rerun. `electron:dev` checks health before spawning the daemon, so it reuses an existing daemon on port `4190`. If the daemon is already running but unhealthy or bound by another process, Electron will report a startup error instead of silently launching against a wrong backend. Build artifacts are under `ui/dist`, `ui/release`, and `target/release`; they can be removed and recreated.

## Artifacts and Notes

Important validation evidence:

    cd ui && bun run test
    Test Files  18 passed (18)
    Tests       72 passed (72)

    cd ui && bun run build
    vite v7.3.1 building client environment for production...
    ✓ built

    cargo build --release --bin ingotd
    Finished `release` profile [optimized]

    cd ui && bun run electron:build
    packaging platform=darwin arch=arm64 electron=42.0.0 appOutDir=release/mac-arm64
    skipped macOS notarization

    test -x ui/release/mac-arm64/Ingot.app/Contents/Resources/bin/ingotd
    bundled-ingotd-executable

    npx asar list ui/release/mac-arm64/Ingot.app/Contents/Resources/app.asar
    /dist
    /electron
    /package.json

## Interfaces and Dependencies

`window.ingotDesktop` has optional fields `apiOrigin` and `wsOrigin`. In packaged mode `apiOrigin` is `ingot://app`, which is handled by the Electron main process and proxies `/api` requests to the daemon. `wsOrigin` is the daemon HTTP origin, currently `http://127.0.0.1:4190`, and renderer helpers convert it to `ws://127.0.0.1:4190/api/ws`. In Vite development the preload exposes no origins so the renderer keeps using Vite's same-origin proxy.

Electron source files are CommonJS (`.cjs`) because `ui/package.json` is an ES module package and Electron can load a CommonJS main entry without a separate TypeScript build step.

Electron dependencies are `electron` for the runtime and `electron-builder` for local packaging. The build metadata packages `dist/**`, `electron/**`, and `package.json`, and adds `../target/release/ingotd` as a resource at `bin/ingotd`. The `electron:build` script sets `CSC_IDENTITY_AUTO_DISCOVERY=false` so local unpacked builds are not blocked by macOS code signing prompts.

Revision note: Initial ExecPlan created before implementation to record the architecture and validation target for the Electron desktop app.

Revision note: Updated after the first Electron build attempt discovered unattended macOS signing can hang. The local build command now disables signing auto-discovery.

Revision note: Updated after runtime review identified HTTP CORS risk from `ingot://app` to the daemon. The packaged app now proxies HTTP API calls through the Electron protocol handler.

Revision note: Updated after final validation to record completed commands and generated artifacts.
