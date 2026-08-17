# Architecture

Four crates, dependencies pointing strictly inwards.

```text
pa-cli    ── parses, wires, dispatches, chooses an exit code
   │
   ├──► pa-app     ── use cases; orchestrates entities through ports
   │        │
   │        └──► pa-domain  ── entities, invariants, ports. No I/O at all.
   │
   └──► pa-infra   ── adapters; implements the ports
            │
            └──► pa-domain
```

`pa-app` and `pa-infra` do not know about each other. They meet exactly once,
in `pa-cli/src/wiring.rs`, which is the only place an `Arc<dyn Port>` is
constructed.

## What lives where

| Crate | Holds | Never holds |
|---|---|---|
| `pa-domain` | `Goal`, `Heartbeat`, `Subagent`, `CronExpr`, `AutonomousState`, and every port trait | `std::fs`, `Command`, `chrono::Utc::now()` |
| `pa-app` | `GoalService`, `SupervisorService`, `HarnessService` … | any concrete adapter, `println!` |
| `pa-infra` | filesystem stores, kernels, CLI runners, gate runner, skill catalog | any business rule |
| `pa-cli` | clap definitions, rendering, composition | any rule a use case could hold |

Time and identity are ports too (`Clock`, `IdGen`), which is why a test can
advance an hour without sleeping and assert on `job-000001`.

## Why the ports exist

Each one earns its place by having at least two real implementations, or by
making a rule testable without the world:

| Port | Implementations |
|---|---|
| `AgentRunner` | `ClaudeRunner`, `OpencodeRunner`, `DryRunner` |
| `RunnerRegistry` | caching, per-session resolution with an honest fallback |
| `KernelPort` | `SocketKernel` (Python, Node), `ShellKernel` |
| `Clock` / `IdGen` | system, and deterministic test doubles |
| `GateRunner` | shell + git fingerprint |
| the stores | JSON-file tables, plus in-memory doubles used by every use-case test |

Adding a third harness is one struct in `pa-infra/src/runners.rs` and one line
in `build()`. Nothing above that file changes.

## How state is stored

```text
~/.prime/agent/
  registry/{sessions,goals,heartbeats,schedules,messages,subagents,…}.json
  sessions/<session-id>.jsonl              append-only transcript
  session-artifacts/<session-id>/
    kernel-state.pkl                       namespace snapshot
    shell-state.json                       cd + exports
    harness/harness_state.json             session-local harness
    refinements.jsonl                      append-only paper trail
  harness/                                 global harness + its paper trail
  runtime/pa_kernel_bootstrap.{py,js}      extracted from the binary
```

Two properties, both about crashes rather than speed:

- **Atomic replace.** Whole-file writes go to a temporary file and are renamed
  over the target, so a process killed mid-write leaves the previous version
  intact rather than a truncated registry.
- **Locked read-modify-write.** `pa` is one-shot, so a cron tick and an
  interactive command genuinely race. `JsonTable::mutate` takes an exclusive
  lockfile, re-reads from disk, applies and writes — the closure sees the state
  that is on disk right now, not a copy read earlier. A lock older than 30
  seconds is broken on the assumption its holder died.

Transcripts and refinement logs are append-only. A corrupt line is skipped
rather than making the rest of the history unreadable.

## The kernel transport

`pa` exits after every command, so the namespace lives in a daemon it starts on
first use:

```text
pa kernel exec ──► UnixStream ──► bootstrap.py (one process, one dict)
                   one JSON line each way
```

The connection is not held between calls — the *namespace* is what persists.
Sockets live in `$XDG_RUNTIME_DIR` under a 16-character hash of
(home, session, language), because `AF_UNIX` paths are capped near 108 bytes and
the natural artifact path exceeds that on any deeply nested workspace. The
bootstrap scripts are embedded in the binary with `include_str!` and rewritten
whenever they differ, so an upgraded `pa` never talks to a stale script.

## Which harness continues a session

A session records the harness that started it, and every later turn goes back to
that one. A `pa tick` from cron does not force every session onto whichever
runner the cron line happened to name — a heartbeat on an opencode session
reaches opencode even when the tick was invoked as `--runner claude`. Where the
recorded harness is not installed on this machine, the registry falls back to
the default rather than stranding the session.

## Trust

The kernel executes model-generated code with your operating-system
permissions. It is a durable control environment, **not a sandbox** — the same
boundary Prime Agent draws. Use an external sandbox for untrusted repositories.
The crates are `#![forbid(unsafe_code)]` throughout, which protects against
memory-safety bugs, not against a program you asked it to run.

## Testing

215 tests, no network, no fixtures directory.

- **Domain** (57): cron arithmetic, budget exhaustion, depth limits, delivery
  rules, output clipping — all pure.
- **Application** (90): every rule against in-memory ports. These are working
  implementations, not assert-on-call mocks, so a passing test is testing the
  rule rather than the plumbing.
- **Infra** (61): real files, real locks, and a real Python kernel — persistence
  across client instances, a traceback that leaves the namespace alive, a
  timeout that does the same, snapshot and restore.
- **CLI** (7): clap's own `debug_assert`, plus exit-code mapping.

```bash
cargo test && cargo clippy --all-targets
```
