---
name: prime-agent
description: Prime Agent's capabilities as one Rust binary, `pa` — a persistent Python/Node/shell kernel whose variables survive between tool calls (prompt-as-a-variable), a continual harness of evidence-backed memories with one-command rollback, real subagents, persistent goals with token budgets, heartbeats, cron schedules, agent-to-agent messaging, bounded autonomous mode with quality gates, and context compaction. Use whenever a task involves large data you would otherwise print into the conversation (logs, JSON, CSV, query results); whenever state must survive across several tool calls or several sessions; whenever you want to remember something durably and be able to undo it; whenever work should continue on a schedule or until a test passes; or whenever you want to delegate to child agents and hear back later. Works from Claude Code, opencode, or a plain shell.
license: MIT
compatibility: Linux or macOS. Rust toolchain to build. python3 (optional, for the Python kernel), node (optional), bash. No network access required.
---

# Prime Agent

One binary, `pa`, that gives any harness the capabilities
[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent) built its
runtime around. State lives on disk under `~/.prime/agent`, so every command is
a fresh process and nothing is lost between them.

## The one idea worth internalising

**Load data into a variable; print only the answer.**

Reading a 200MB log into the conversation costs tokens proportional to its
size. Loading it into the kernel and printing `len(errors)` costs eleven. The
kernel is a separate long-lived process, so the variable is still there on your
next tool call — and the one after that.

```bash
pa kernel exec 'rows = [json.loads(l) for l in open("server.log")]'   # nothing printed
pa kernel exec 'len(rows)'                                            # 4213908
pa kernel exec 'from collections import Counter
Counter(r["path"] for r in rows if r["status"] >= 500).most_common(3)'
```

Three calls, one load, and the conversation never sees a single row. Use
`pa kernel vars` to see what is bound — names, types and sizes, never values.

## Setup

```bash
cd ~/.agents/skills/prime-agent && ./install.sh
```

Builds the release binary and links it into `~/.local/bin/pa`. Run
`pa status` to confirm; it reports which harness will run agent turns.

## Command map

Everything below has `--json` for machine-readable output and `--session <name>`
to act on a session other than the default (one per working directory,
auto-created).

| Need | Command | Detail |
|---|---|---|
| Keep data out of context | `pa kernel exec` / `vars` / `reset` | [references/kernel.md](references/kernel.md) |
| Remember something durably | `pa harness remember` / `list` | [references/harness.md](references/harness.md) |
| Undo something you remembered | `pa refine list` / `rollback <id>` | [references/harness.md](references/harness.md) |
| Delegate to child agents | `pa agent spawn` / `list` / `settle` | [references/subagents.md](references/subagents.md) |
| Talk between agents | `pa send` / `pa inbox` / `pa roster` | [references/subagents.md](references/subagents.md) |
| Keep an objective across turns | `pa goal set` / `status` / `complete` | [references/continuity.md](references/continuity.md) |
| Check in on a timer | `pa heartbeat set` / `add` / `list` | [references/continuity.md](references/continuity.md) |
| Run a prompt later or on cron | `pa schedule add` / `list` | [references/continuity.md](references/continuity.md) |
| Work until the tests pass | `pa autonomous on --gate` / `step` | [references/continuity.md](references/continuity.md) |
| Deliver everything that is due | `pa tick` | [references/continuity.md](references/continuity.md) |
| Shrink a long session | `pa compact status` / `run` | `pa compact --help` |
| See installed skills | `pa skills list` / `prompt` | reads Claude, opencode and `.agents` dirs |
| Get the supplemental prompt | `pa prompt` | harness + skills + live goal, one block |

## When to reach for this

- **Any task where you would print a lot to read a little.** Logs, JSON dumps,
  CSVs, API responses, search results. Load, then query.
- **State that must outlive the tool call.** A parsed structure, a running
  total, a half-built index. `pa kernel exec` keeps it; re-deriving it does not.
- **A lesson worth keeping.** `pa harness remember` demands evidence and records
  a reversible change, so a wrong lesson is one command to undo.
- **Work that outlives the conversation.** A goal with a token budget, a
  heartbeat that checks a deploy, a cron prompt for Monday morning.
- **Fan-out.** `pa agent spawn` admits a child and returns immediately; you end
  your turn and read the reply from `pa inbox` later.

## Two rules the tool enforces, and why

**Evidence before memory.** `pa harness remember` refuses an entry with no
`--evidence`. An entry nobody can justify later is an entry nobody can safely
delete, and a harness full of those is worse than an empty one.

**"Out of budget" is never reported as "done".** A goal that exhausts its token
budget ends as `budget-exhausted`, and `pa goal complete` then fails. Only
finishing the work marks it complete. The same distinction runs through
autonomous mode: `finish` means the gates passed, `stop` means a limit was hit.

## Exit codes

`0` success · `1` ran fine, answer was negative (gate failed, code raised) ·
`2` autonomous mode wants another turn · `64` bad input · `65` state forbids it
· `69` not installed · `70` the world failed.

So a self-driving loop is just:

```bash
while pa autonomous step; [ $? -eq 2 ]; do claude -p "$(pa prompt)"; done
```

## Notes

- Kernels start lazily and exit after an hour idle. `pa kernel stop` ends one now.
- `PA_RUNNER=claude|opencode|dry-run` picks the harness; `dry-run` reports what
  would happen without spending anything.
- `PA_KERNEL=python|node|shell` (or `--lang`) picks the namespace. The shell
  kernel persists `cd` and `export` only — it holds no objects, and says so.
- Full architecture and the reasoning behind each boundary:
  [references/architecture.md](references/architecture.md).
