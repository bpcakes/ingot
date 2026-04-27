# Ingot

Ingot is a local control plane for shipping AI-authored code without handing your repository over to an unsupervised chat session.

It sits between agent CLIs such as Codex and Claude Code and your real local Git repositories. Agents can author, review, and investigate, but Ingot owns the durable workflow state, worktrees, validation commands, findings, approvals, convergence, and final ref updates. The result is a repeatable delivery lane: every item has a trace, every job has logs, and the target branch only moves after the integrated result passes the gates you configured.

Use Ingot when you want AI agents to do real engineering work in local repos, while keeping Git, validation, and human authority outside the agent's control.

## What You Get

- **A project board for AI delivery.** Register local repositories, create delivery or investigation items, and watch each item move through inbox, working, approval, and done.
- **Supervised agent jobs.** Ingot launches Codex or Claude Code as bounded subprocesses with explicit prompts, schemas, workspaces, timeouts, logs, and structured results.
- **Separate author, review, and validation gates.** Agent-authored commits are reviewed, validated by your harness commands, repaired within budgets, rebased onto the live target, and validated again after integration.
- **Investigation-to-delivery flow.** Investigation items produce structured findings, which can be triaged, skipped, backlogged, or promoted into delivery items.
- **Manual or autopilot operation.** Run each step explicitly, or let autopilot dispatch safe workflow steps until it reaches approval, triage, conflict, or escalation.
- **Git-first finalization.** Ingot prepares integration commits in managed worktrees, queues convergence by target ref, and advances the final ref only when the prepared result is still valid.
- **Operational auditability.** The UI exposes jobs, live output, workspaces, findings, convergences, diagnostics, config, and project activity. SQLite stores the durable state under `~/.ingot/`.

## Why It Exists

Agent CLIs are good at producing patches. They are less good at being the system of record for whether work is safe to ship. Ingot treats agents as replaceable workers inside a stricter local delivery system:

- Agents edit files; Ingot creates canonical commits and records job outcomes.
- Agents report findings; Ingot owns triage, promotion, approval, and rework decisions.
- Agents can fail, hang, or return malformed output; Ingot records the failure and recovers conservatively.
- Target branches can move; Ingot replays onto the current target and checks that the prepared result is still valid before finalization.

Ingot is not a general task tracker or workflow engine. Its scope is intentionally narrow: single-item code delivery and investigation in real local Git repositories.

## How It Works

You give Ingot a work item with a title, description, and acceptance criteria. For delivery items, it drives the item through this pipeline:

1. **Author** code changes in an isolated Git worktree using a supervised AI agent.
2. **Review** the incremental diff and the full candidate through structured read-only agent jobs.
3. **Validate** the candidate by running the repository's declared harness commands.
4. **Repair** findings and failed validation through bounded rework loops.
5. **Prepare convergence** by replaying the candidate onto the current target branch.
6. **Validate integration** against the rebased result.
7. **Finalize** the target ref only after all gates pass and any required human approval is granted.

Investigation items use a lighter read-only workflow: they inspect the repository, emit structured findings, and let you promote real issues into delivery items.

Every step is durable, auditable, and recoverable. If the daemon crashes mid-operation, it reconciles from SQLite state, workspaces, and the Git operation journal on restart, assuming uncertainty rather than inventing success.

## Key Design Decisions

- **Items are durable.** A work item survives retries, rework loops, approval rejection, revision changes, defer/resume, and manual terminal decisions.
- **Revisions freeze meaning.** Changing a title, description, or acceptance criteria creates a new revision. In-flight work is never silently rewritten.
- **Jobs are bounded.** Every agent job is a subprocess with explicit inputs, output schema, workspace, permission level, timeout, and logs.
- **Git truth belongs to the daemon.** Agents edit files. The daemon creates canonical commits with audit trailers, owns scratch refs, and moves the target ref via compare-and-swap.
- **Convergence is explicit and two-stage.** Authoring success does not imply integration success. Prepare replays the commit chain onto the current target, then finalize CAS-updates the ref.
- **Human authority is first-class.** Human commands outrank late or stale agent events. Approval, escalation, defer, dismiss, and rework are explicit state transitions.
- **Conservative recovery.** If there is uncertainty, the system assumes failure. The Git operation journal enables crash recovery without inventing success.

## Architecture

Two processes:

- A **Rust daemon** (`ingotd`) that owns orchestration, persistence, workspaces, Git, recovery, and agent execution
- A **React SPA** that presents live state over REST and WebSocket

```
┌─────────────────────────────────────────────────────────────┐
│                         React UI                            │
│                   (Vite, TypeScript)                         │
│                                                             │
│  Project Switcher                                           │
│  ├─ Dashboard                                               │
│  ├─ Board (items only)                                      │
│  ├─ Item Detail / Revision / Workspace                      │
│  ├─ Jobs                                                    │
│  └─ Config                                                  │
└────────────────┬──────────────────────────┬─────────────────┘
                 │ HTTP (REST)              │ WebSocket
                 │ commands + queries       │ live state push
┌────────────────┴──────────────────────────┴─────────────────┐
│                       Rust Daemon                           │
│                                                             │
│  Workflow Evaluator ── Dispatcher / Job Runner ── Git Mgr   │
│         │                      │                    │       │
│  Item Projection ──── Convergence Manager ── Agent Runtime  │
│         │                      │                    │       │
│     SQLite ──────── Activity / Observability ── CLI Procs   │
└─────────────────────────────────────────────────────────────┘
```

### Rust Workspace (13 crates)

| Crate | Responsibility |
|---|---|
| `ingot-domain` | Pure entities, enums, invariants, repository port traits, domain events |
| `ingot-workflow` | Workflow graph, step contracts, pure evaluator (projects state, never mutates) |
| `ingot-usecases` | Command handlers, transaction boundaries, daemon-only system actions |
| `ingot-store-sqlite` | SQLite repository implementations and migrations |
| `ingot-git` | Git operations via `tokio::process`—commits, refs, convergence replay |
| `ingot-workspace` | Worktree provisioning, reset, reuse, and cleanup |
| `ingot-agent-protocol` | `AgentAdapter` trait, request/response types, result schemas |
| `ingot-agent-adapters` | Claude Code and Codex adapter implementations |
| `ingot-agent-runtime` | Subprocess spawning, supervision, heartbeats, log capture |
| `ingot-config` | YAML config loading with global/project merge |
| `ingot-http-api` | Axum routes, DTOs, WebSocket transport |
| `ingot-daemon` | Binary wiring only—DI, config bootstrap, signal handling |
| `ingot-test-support` | Shared fixtures for Rust integration tests |

Hard dependency rules: `ingot-domain` and `ingot-workflow` must never depend on sqlx, axum, or tokio::process. `ingot-usecases` depends on ports, not infrastructure.

### UI

React + TypeScript SPA. Zustand tracks local connection state, TanStack Query handles server data with WebSocket-driven cache invalidation, and Tailwind CSS plus local UI primitives render the app.

The UI includes:

- Projects and demo project creation
- Dashboard and board views
- Item detail with workflow stepper, operator actions, jobs, findings, convergences, diagnostics, and activity timeline
- Jobs page with live streamed output, prompt snapshots, and structured results
- Workspaces page for retained worktree inspection and recovery actions
- Activity and config pages for audit events, execution mode, agent routing, auto-triage policy, and agent health

## Prerequisites

- Rust stable (1.85+)
- [Bun](https://bun.sh) (for UI package management and scripts)
- SQLite 3
- Git
- At least one supported AI agent CLI installed:
  - [Claude Code](https://docs.anthropic.com/en/docs/claude-code) (`claude`)
  - [Codex](https://github.com/openai/codex) (`codex`)

## Getting Started

```sh
# Clone
git clone https://github.com/featherenvy/ingot.git
cd ingot

# Install UI dependencies
make ui-install

# Run both daemon and UI dev server
make dev
```

The daemon serves on `:4190` and the UI dev server on `:4191`. Open `http://localhost:4191` to register a real local repository or create a demo project.

On startup, the daemon probes default `codex` and `claude` CLIs and registers any available agents. You can also add or reprobe agents from the Config page.

### Register a project

Once the daemon is running, register a local Git repository through the API:

```sh
curl -X POST http://localhost:4190/api/projects \
  -H "Content-Type: application/json" \
  -d '{"name": "my-project", "path": "/path/to/repo", "default_branch": "main"}'
```

### Configure verification (optional)

Add a harness profile to your repository to enable automated build/test/lint validation:

```toml
# <repo>/.ingot/harness.toml

[commands.build]
run = "make build"
timeout = "5m"

[commands.test]
run = "make test"
timeout = "10m"

[commands.lint]
run = "make lint"
timeout = "2m"

[skills]
paths = [".ingot/skills/*.md"]
```

## Development

```sh
make help             # Show all available targets

make check            # Type-check Rust workspace
make test             # Run Rust tests
make lint             # All linters: clippy + biome + fmt check
make build            # Build Rust workspace
make all              # check + test + lint + build

make ui-build         # Typecheck + vite build
make ui-test          # Vitest
make ui-lint          # Biome check

make dev              # Daemon (:4190) + UI dev server (:4191)
make dev-daemon       # Daemon only
make dev-ui           # UI only
```

Run a single Rust test:

```sh
cargo test -p ingot-workflow test_name
```

Run a single UI test:

```sh
cd ui && bunx vitest run src/test/board.test.ts
```

## Configuration

Configuration is YAML. A global config is loaded first; a project-local config overrides it when present:

```
~/.ingot/config.yml          # Global defaults
<repo>/.ingot/config.yml     # Per-project override
```

Prompt templates are built into the daemon. Harness commands and repo-local skill files can be configured per project in `<repo>/.ingot/harness.toml`.

## Operational Footprint

```
~/.ingot/
├── ingot.db          # SQLite runtime state
├── config.yml
├── repos/<project_id>.git
├── worktrees/<project_id>/
├── logs/daemon.log
└── logs/<job_id>/
    ├── prompt.txt
    ├── output.jsonl
    ├── stdout.log
    ├── stderr.log
    └── result.json

<repo>/.ingot/
├── config.yml
├── harness.toml
└── skills/*.md
```

## Formal Verification

The `formal/` directory contains TLA+ specifications for critical control properties. Run model checking with:

```sh
make tla-check
```

## Documentation

- [SPEC.md](./SPEC.md) — Normative service specification (runtime behavior, entity invariants, command semantics, recovery rules)
- [ARCHITECTURE.md](./ARCHITECTURE.md) — Non-normative implementation shape (module boundaries, design rationale, tech stack)

## License

MIT
