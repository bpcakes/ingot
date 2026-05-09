# Harden the Electron desktop app after review

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. This document must be maintained in accordance with `.agent/PLANS.md`.

## Purpose / Big Picture

The Electron app now packages the existing React frontend and Rust `ingotd` backend into a desktop shell, but a fresh review found four reliability issues. After this plan is implemented, desktop startup failures should be reported as clear errors instead of process crashes, development startup should never attach to the wrong Vite server, custom backend origins should be handled consistently, and the release packaging command should not unexpectedly hang on macOS signing auto-discovery.

The working behavior to demonstrate is: `cd ui && bun run electron:build` still creates an unpacked app with a bundled `ingotd`; `cd ui && bun run electron:dev` either opens against the Vite server it started or fails clearly when port `4191` is unavailable; and setting `INGOT_API_ORIGIN=http://127.0.0.1:4190/` still makes health and proxied API requests target `/api/...`, not `//api/...`.

## Progress

- [x] (2026-05-08 00:00Z) Reviewed the newly added Electron code and identified four concrete issues to address.
- [x] (2026-05-08 00:00Z) Wrote this ExecPlan from the review findings.
- [x] (2026-05-08 20:42Z) Implemented daemon spawn error handling in `ui/electron/main.cjs`, including asynchronous `error` handling and early-exit rejection before health is ready.
- [x] (2026-05-08 20:42Z) Added `ui/electron/daemon-url.cjs` and switched health/proxy URL construction to `URL`-based helpers that normalize `INGOT_API_ORIGIN`.
- [x] (2026-05-08 20:42Z) Made `electron:dev` strict about the Vite port by adding `--strictPort` to the Vite child process arguments.
- [x] (2026-05-08 20:42Z) Made `electron:dist` deterministic for local unsigned builds by adding `CSC_IDENTITY_AUTO_DISCOVERY=false`.
- [x] (2026-05-08 20:42Z) Added focused Node tests and a `bun run electron:test` script for Electron helper behavior and CommonJS syntax checks.
- [x] (2026-05-08 20:45Z) Ran validation commands and recorded results here.

## Surprises & Discoveries

- Observation: `child_process.spawn()` reports missing commands and permission problems through an asynchronous `error` event, not a synchronous throw.
  Evidence: `ui/electron/main.cjs` currently calls `spawn()` inside `ensureDaemon()` but only attaches `stdout`, `stderr`, and `exit` handlers. Without an `error` handler, a missing binary can become an unhandled event.
- Observation: Vite can choose a different port if the requested port is occupied unless strict port mode is enabled.
  Evidence: `ui/electron/dev.cjs` starts `bun run dev -- --host 127.0.0.1` and then waits for the hard-coded `http://127.0.0.1:4191`; `ui/vite.config.ts` sets `server.port = 4191` but does not set `strictPort`.
- Observation: `INGOT_API_ORIGIN` is read as a raw string in the Electron main process.
  Evidence: `ui/electron/main.cjs` currently computes `HEALTH_URL` as `${API_ORIGIN}/api/health` and proxy targets as `${API_ORIGIN}${url.pathname}${url.search}`.
- Observation: Local macOS signing already blocked an unattended Electron build once.
  Evidence: The previous first `electron:build` run reached `codesign` and had to be terminated; the current `electron:build` sets `CSC_IDENTITY_AUTO_DISCOVERY=false`, but `electron:dist` does not.
- Observation: `INGOT_API_ORIGIN` is clearer and safer when treated as a true origin, not an arbitrary base URL.
  Evidence: `ui/electron/daemon-url.cjs` now accepts root-only HTTP(S) origins with optional trailing slashes and rejects paths, queries, fragments, credentials, and unsupported protocols before the main process starts proxying requests.
- Observation: Vitest's default discovery includes CommonJS `*.test.cjs` files outside `src/test`.
  Evidence: The first `cd ui && bun run test` run executed `electron/daemon-url.test.cjs` with Vitest and failed with `No test suite found`; `ui/vite.config.ts` now limits Vitest to `./src/test/**/*.{test,spec}.{ts,tsx}`, leaving Electron CommonJS tests to `bun run electron:test`.
- Observation: `--strictPort` prevents Vite from choosing another port, but the Electron dev wrapper also needs to avoid polling an already-running dummy server on `4191`.
  Evidence: `ui/electron/dev.cjs` now checks the default Vite URL with a TCP connection before spawning Vite. The occupied-port validation exited with code 1 and printed `Port 4191 is already in use at http://127.0.0.1:4191; electron:dev requires its own Vite server.`

## Decision Log

- Decision: Treat these as reliability fixes to the existing Electron architecture rather than changing the architecture.
  Rationale: The packaged app already builds and contains the right assets; the risks are lifecycle, URL normalization, and script semantics.
  Date/Author: 2026-05-08 / Codex.
- Decision: Keep the daemon on `127.0.0.1:4190` and continue using Electron's `ingot://app/api` proxy for packaged HTTP requests.
  Rationale: This avoids adding CORS behavior to the Rust backend and preserves the standalone backend/frontend workflows.
  Date/Author: 2026-05-08 / Codex.
- Decision: Make `electron:dev` use Vite strict-port behavior instead of parsing a dynamically chosen port.
  Rationale: The app and docs already standardize on port `4191`; strict failure is safer than opening a window against an unrelated server.
  Date/Author: 2026-05-08 / Codex.
- Decision: Make unsigned packaging the default for both local Electron packaging scripts unless a separate signed release path is intentionally introduced later.
  Rationale: The repository needs reliable local commands first. Signed distribution requires credentials, notarization choices, and CI secrets that are outside this hardening task.
  Date/Author: 2026-05-08 / Codex.
- Decision: Extract daemon URL normalization into `ui/electron/daemon-url.cjs` and cover it with Node's built-in test runner.
  Rationale: The Electron main process is CommonJS and outside the existing Vitest TypeScript pipeline, so a small CommonJS helper gives regression coverage without adding a build step or new dependencies.
  Date/Author: 2026-05-08 / Codex.
- Decision: Add a preflight occupied-port check to `electron:dev` for the default Vite URL before starting either child process.
  Rationale: Strict Vite port mode is necessary but not sufficient on its own because the wrapper's HTTP polling could otherwise see an unrelated server before Vite has finished failing. The preflight check gives an immediate clear failure and prevents Electron from opening against that server.
  Date/Author: 2026-05-08 / Codex.

## Outcomes & Retrospective

Implementation and validation are complete. The Electron main process now normalizes `INGOT_API_ORIGIN` through `ui/electron/daemon-url.cjs`, constructs health and proxied API URLs with `URL`, handles daemon `spawn` errors and early exits before health is ready, and preserves daemon reuse and owned-process shutdown. The development wrapper now refuses to start when the default Vite port is already occupied and still passes `--strictPort` to Vite. `electron:dist` now has the same `CSC_IDENTITY_AUTO_DISCOVERY=false` guard as `electron:build`. Focused Electron helper tests pass, the UI test/lint/build gates pass, `electron:build` completes with ad-hoc macOS signing and skipped notarization, and the packaged macOS app contains an executable `Contents/Resources/bin/ingotd`. The remaining release-signing work is intentionally outside this hardening task and should be added later as a separate signed release command with credentials and notarization choices.

## Context and Orientation

The existing backend is the Rust daemon binary `ingotd`, defined in `apps/ingot-daemon/src/main.rs`. It binds `127.0.0.1:4190`, exposes `/api/health`, `/api/ws`, and the rest of the HTTP API through `crates/ingot-http-api`, and uses the same route shape whether started standalone or from Electron.

The frontend is the React/Vite app under `ui/`. During normal web development, `ui/vite.config.ts` serves the frontend on port `4191` and proxies `/api` to the daemon. In the Electron app, `ui/electron/main.cjs` is the Electron main process. The main process creates the desktop window, starts or reuses `ingotd`, serves built frontend files from the custom `ingot://app/` protocol, and proxies packaged HTTP API requests from `ingot://app/api/...` to the daemon. `ui/electron/preload.cjs` exposes a tiny `window.ingotDesktop` object to the renderer. `ui/src/api/base.ts` uses that object to choose HTTP and WebSocket URLs.

An Electron main process is Node.js code that owns native windows and child processes. A renderer is the browser-like process that runs the React UI. A preload script runs between them and should expose only the minimal data the renderer needs.

## Plan of Work

First, edit `ui/electron/main.cjs` so daemon URL handling is centralized and safe. Add a helper such as `normalizeOrigin(value)` that strips trailing slashes and validates that the value is an HTTP or HTTPS origin. Add `daemonUrl(pathnameAndSearch)` or `apiUrl(pathname, search)` using `new URL()` so `/api/health` and proxied `/api/...` targets are correct even if `INGOT_API_ORIGIN` contains a trailing slash. Replace the top-level `HEALTH_URL` string concatenation and the proxy string concatenation with this helper. If validation fails, throw a clear error that includes `INGOT_API_ORIGIN`.

Second, still in `ui/electron/main.cjs`, make `ensureDaemon()` reject on spawn failure. Wrap the child process startup in a promise or attach `daemonProcess.once('error', ...)` before waiting for health. If `spawn()` emits `error`, clear `daemonProcess` and throw a message such as `Unable to launch ingotd: <system message>`. Preserve the existing behavior of reusing an already healthy daemon and killing only the daemon process that Electron started. If the daemon exits before health becomes available, fail fast with the exit code or signal instead of waiting for the full health timeout.

Third, edit `ui/electron/dev.cjs` so development cannot silently attach to an unrelated server. For the default `http://127.0.0.1:4191` URL, check that the TCP port is free before spawning Vite. Change the Vite invocation to include `--strictPort`, for example `bun run dev -- --host 127.0.0.1 --strictPort`. Keep waiting for `http://127.0.0.1:4191`, but if the port is occupied or the Vite child exits, the parent script should exit with a non-zero status. The existing child exit handler already exits when any child exits unexpectedly; preserve that behavior.

Fourth, update `ui/package.json` scripts. Keep `electron:build` as the reliable unsigned local build command. Also decide whether `electron:dist` should be an unsigned local distributable command or a signed release command. For this plan, make it deterministic by adding `CSC_IDENTITY_AUTO_DISCOVERY=false` to `electron:dist` as well. If a signed release command is desired later, add a separate name such as `electron:dist:signed` and document that it requires configured signing credentials. Do not add credentials or notarization secrets in this task.

Fifth, add focused tests or script checks. Because the Electron main files are CommonJS and not part of the TypeScript/Vitest pipeline, prefer extracting pure helpers into a small CommonJS module such as `ui/electron/daemon-url.cjs`, then test it with Node's built-in test runner in `ui/electron/daemon-url.test.cjs`. Cover default origin, trailing slash origin, invalid origin, health URL construction, and proxy path/search construction. Add a package script such as `electron:test` that runs `node --test electron/*.test.cjs` and `node --check electron/main.cjs electron/preload.cjs electron/dev.cjs`. If extracting a module feels too broad, at minimum add `node --check` validation and manually validate trailing slash behavior with a short one-off Node command, but prefer actual tests.

Update `.agent/electron-desktop-app.md` only if implementation decisions supersede what that existing plan says. Keep this hardening plan current as changes are made.

## Concrete Steps

From the repository root, inspect the current Electron files:

    sed -n '1,280p' ui/electron/main.cjs
    sed -n '1,140p' ui/electron/dev.cjs
    sed -n '1,120p' ui/package.json

Edit `ui/electron/main.cjs` and optionally add `ui/electron/daemon-url.cjs` plus `ui/electron/daemon-url.test.cjs`. The URL helper should make these cases true:

    default origin + /api/health -> http://127.0.0.1:4190/api/health
    http://127.0.0.1:4190/ + /api/health -> http://127.0.0.1:4190/api/health
    http://127.0.0.1:4190/ + /api/projects?limit=1 -> http://127.0.0.1:4190/api/projects?limit=1
    not-a-url -> clear validation error

Edit `ui/electron/dev.cjs` so the default Vite port is checked before child processes start and the Vite spawn arguments include `--strictPort`.

Edit `ui/package.json` so local packaging commands are deterministic. Add `electron:test` if helper tests are added. Make `electron:dist` either explicitly unsigned or split out a separate signed release command; for this plan, use unsigned by default.

Run the validation commands from the repository root:

    cd ui && bun run electron:test
    cd ui && bun run test
    cd ui && bun run lint
    cd ui && bun run build
    cd ui && bun run electron:build

If `electron:test` is not added, replace it with:

    cd ui && node --check electron/main.cjs && node --check electron/preload.cjs && node --check electron/dev.cjs

After `electron:build`, confirm the bundled daemon is present:

    test -x ui/release/mac-arm64/Ingot.app/Contents/Resources/bin/ingotd && echo bundled-ingotd-executable

On non-macOS platforms, adjust the release app path to the platform-specific Electron Builder output directory and record the actual path in this plan.

## Validation and Acceptance

Acceptance requires evidence for each review finding:

For daemon spawn failure handling, simulate a bad packaged daemon origin or missing command in a way that does not damage the repository. One safe check is to temporarily run Electron with a bad `PATH` in development so `cargo` cannot be found, or to test the extracted spawn wrapper if it is made injectable. The expected result is a controlled error path, not an unhandled `error` event. At minimum, the code must attach an `error` handler before waiting for health and the review must confirm that path rejects with a clear message.

For strict Vite port behavior, inspect `ui/electron/dev.cjs` and confirm the Vite child process includes `--strictPort`. If practical, start a dummy server on `127.0.0.1:4191` and run `cd ui && bun run electron:dev`; the expected result is that the script exits rather than opening Electron against the dummy server. Do not leave the dummy server running.

For `INGOT_API_ORIGIN` normalization, run the helper tests. They must prove that both `http://127.0.0.1:4190` and `http://127.0.0.1:4190/` produce the same `/api/...` URLs and that invalid input fails clearly.

For signing-script determinism, inspect `ui/package.json` and run `cd ui && bun run electron:build`. It must complete without interactive signing prompts. If `electron:dist` is kept as a local command, it must also include the same signing auto-discovery guard or have a separate documented signed-release command.

For regression coverage, `cd ui && bun run test`, `cd ui && bun run lint`, and `cd ui && bun run build` must pass. `cd ui && bun run electron:build` must still produce an app bundle with executable `Contents/Resources/bin/ingotd` on macOS.

## Idempotence and Recovery

All edits are source-level and safe to repeat. The Electron build writes under `ui/release`, which is ignored by Git and can be removed with `rm -rf ui/release`. The UI build writes under `ui/dist`, which is also ignored by Git. If a validation run starts a dummy process to occupy port `4191`, record its PID and kill it before finishing. Do not use `git reset --hard` or remove unrelated untracked files.

If `electron:dev` is run and leaves a Vite or Electron process behind after an interrupted test, find it with:

    ps -axo pid,ppid,command | rg 'vite|electron|ingotd'

Then kill only the process started for the test. Do not kill an unrelated user-running daemon unless it is clearly the one launched for validation.

## Artifacts and Notes

The original review findings this plan addresses are:

    P2: unhandled daemon spawn failures can crash Electron instead of showing the startup dialog.
    P2: electron:dev can load the wrong server if port 4191 is occupied.
    P2: INGOT_API_ORIGIN is not normalized in the Electron main process.
    P3: electron:dist can repeat the signing hang already seen in electron:build.

Final command transcripts from this implementation:

    cd ui && bun run electron:test
    tests 8
    pass 8

    cd ui && bun run test
    Test Files 18 passed (18)
    Tests 72 passed (72)

    cd ui && bun run lint
    Checked 108 files in 20ms. No fixes applied.

    cd ui && bun run build
    vite v7.3.1 building client environment for production...
    built in 2.26s

    cd ui && node -e "<temporary dummy server on 127.0.0.1:4191, then bun run electron:dev>"
    Error: Port 4191 is already in use at http://127.0.0.1:4191; electron:dev requires its own Vite server.
    electron-dev-exit-code 1

    cd ui && bun run electron:build
    packaging platform=darwin arch=arm64 electron=42.0.0 appOutDir=release/mac-arm64
    falling back to ad-hoc signature for macOS application code signing
    skipped macOS notarization

    test -x ui/release/mac-arm64/Ingot.app/Contents/Resources/bin/ingotd && echo bundled-ingotd-executable
    bundled-ingotd-executable

## Interfaces and Dependencies

If adding a URL helper module, define it in `ui/electron/daemon-url.cjs` as CommonJS so `ui/electron/main.cjs` can require it without a build step. It should export functions with stable names:

    normalizeApiOrigin(value)
    daemonApiUrl(apiOrigin, pathname, search)

`normalizeApiOrigin(value)` accepts a string or undefined and returns an origin string with no trailing slash. If `value` is undefined, use `http://127.0.0.1:4190`. It should reject unsupported protocols, invalid URLs, credentials, and any non-root path, query, or fragment with an `Error` that names `INGOT_API_ORIGIN`.

`daemonApiUrl(apiOrigin, pathname, search)` returns a URL string. `pathname` must be `/api` or begin with `/api/`; reject any path outside `/api` so the protocol proxy cannot accidentally forward non-API renderer requests to the daemon. `search` is optional and should preserve query strings exactly as received from `new URL(request.url).search`.

`ui/electron/main.cjs` must depend on these helpers for `HEALTH_URL` and `proxyApi()` target construction. `ui/electron/preload.cjs` and `ui/src/api/base.ts` do not need behavior changes unless implementation reveals a mismatch.

Revision note: Initial hardening ExecPlan created from the four review findings after the Electron desktop app implementation.
Revision note: Implemented the hardening work, added focused Electron URL tests, constrained Vitest discovery to `src/test`, added an occupied-port preflight for `electron:dev`, and recorded the validation results so a future reader can see both the source changes and the observed behavior.
