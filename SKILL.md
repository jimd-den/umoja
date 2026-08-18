---
name: umoja
description: UMOJA (Umoja Manages Orchestrated Joint Agents) — one Rust binary, `pa`, giving any harness a persistent Python/Node/shell kernel whose variables survive between tool calls (prompt-as-a-variable), a file toolkit that searches and edits without printing files into the conversation, blocking and fire-and-forget subagents (`rlm`), an evidence-backed harness of memories with one-command rollback and an automated review pass, persistent goals with token budgets, heartbeats, cron schedules, agent-to-agent messaging, bounded autonomous mode with quality gates, and context compaction. Use whenever a task involves reading or searching more than one or two files; whenever there is large data you would otherwise print into the conversation (logs, JSON, CSV, query results); whenever state must survive across several tool calls or several sessions; whenever you want to remember something durably and be able to undo it; whenever work should continue on a schedule or until a test passes; or whenever you want to delegate to child agents. Works from Claude Code, opencode, or a plain shell.
license: MIT
compatibility: Linux or macOS. Rust toolchain to build. python3 (optional, for the Python kernel), node (optional), bash. No network access required.
---

# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents. *Umoja* is Swahili
for "unity" — one namespace, many agents, pulling in one direction.

## While this skill is active, the kernel is how you touch files

This is the point of invoking UMOJA, and it is not a preference. Once this
skill is on:

| Instead of | Use |
|---|---|
| `Read` a file | `pa kernel exec 'print(head("path"))'`, or `outline`, or `slice_lines` |
| `Read` several files | `load("src/**/*.rs")` once, then query |
| `Grep` / `Glob` | `grep("pattern")` over what you loaded |
| `Write` a file | `write("path", text)` |
| `Edit` a file | `edit("path", old, new)` |
| `Bash` for a command whose output you then read | `sh("cmd")`, so the output lands in a variable |

**Why the rule is exclusive rather than advisory.** Reading a file with `Read`
puts its entire text in the conversation, permanently, whether or not the task
needed more than four lines of it. That is the cost UMOJA exists to avoid, and
it cannot be avoided halfway: a session that loads a tree into the kernel and
*then* reads six files with `Read` has paid the full price of both. The
namespace only pays off if it is the single door files come through.

**The one honest exception.** Claude Code's own `Edit` tool requires that its
`Read` tool has seen the file first. When you are editing through the harness's
`Edit`, that `Read` is mandatory and correct — do not fight it. Use the
kernel's `edit`/`write` when you are working *in* the kernel, and the harness's
Read+Edit pair when you are working through the harness. What is never right is
`Read`-ing files merely to *look* at them while the kernel is sitting there
loaded.

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

### The failure mode this skill is designed around

Loading a hundred files and then printing them back out costs exactly what
reading them one at a time would have cost. The namespace added a step and
saved nothing. This is *the* easy mistake, and it is easy because
`print(FILES[path])` is one short line.

So the short lines are the reducing ones. Before any call that prints, ask:
**am I about to print something I could search, count, or slice first?**

## The file toolkit

Bound in every Python kernel, no import needed. `pa kernel vars` hides them, so
your own bindings stay legible.

```bash
pa kernel exec 'load("crates/**/*.rs")'          # 57 files, 511.6 KB in FILES
pa kernel exec 'for h in grep("pub fn call"): print(h)'
pa kernel exec 'print("\n".join(outline("src/lib.rs")))'
```

| Call | Returns |
|---|---|
| `load(pattern, root=".")` | Reads a glob into `FILES`. Prints a tally, never contents. Unreadable files are skipped and counted. |
| `grep(pattern, where=None, context=0, limit=200, ignore_case=False)` | `path:line: text` matches — the answer, not the haystack. |
| `outline(path)` | A file's definitions, so you can decide where to look without reading the way there. |
| `head(path, lines=40)` | The first N lines, for when a peek really is enough. |
| `slice_lines(path, start, end)` | Lines `start`..`end`, 1-indexed, inclusive. |
| `write(path, text)` | Writes, and refreshes `FILES` so the next `grep` sees it. |
| `edit(path, old, new, count=1)` | Exact replacement. **Refuses** when `old` is missing or ambiguous rather than guessing. |
| `sh(command, cwd=None)` | `(exit_code, stdout, stderr)`, so a command's output stays in the namespace. |

`outline`, `head`, `slice_lines` and `edit` fall back to reading from disk when
a path was never loaded — being told "no definitions" because of a stale key is
a wrong answer wearing a right answer's clothes.

## Recursive delegation: `rlm(...)`

Inside the kernel, `rlm` asks a child agent a question and hands back its
answer as a string. This is the recursive half of prompt-as-a-variable: the
namespace keeps a large *input* out of the conversation, and `rlm` keeps a
large *sub-task* out of it too.

```bash
pa kernel exec '
verdicts = {p: rlm(f"Breaking change in {p}? yes/no + why") for p in changed}
sum(1 for v in verdicts.values() if v.lower().startswith("yes"))'
```

It blocks — that is the trade. A failed child raises `Delegation` rather than
returning its error text, because an error that reads like an answer is worse
than no answer. From the shell the same thing is `pa agent call "..."`, which
exits non-zero when the child failed.

**When not to block:** if the answer is not needed before the turn ends, use
`pa agent spawn`, which returns the instant the child is admitted and lets
several run at once. Replies arrive via `pa inbox` or files.

## Command map

Everything below has `--json` for machine-readable output and `--session <name>`
to act on a session other than the default (one per working directory,
auto-created).

| Need | Command | Detail |
|---|---|---|
| Keep data out of context | `pa kernel exec` / `vars` / `reset` | [references/kernel.md](references/kernel.md) |
| Delegate and wait | `pa agent call` — or `rlm(...)` in the kernel | [references/subagents.md](references/subagents.md) |
| Delegate and carry on | `pa agent spawn` / `list` / `settle` | [references/subagents.md](references/subagents.md) |
| Talk between agents | `pa send` / `pa inbox` / `pa roster` | [references/subagents.md](references/subagents.md) |
| Remember something durably | `pa harness remember` / `list` | [references/harness.md](references/harness.md) |
| Ask what is worth remembering | `pa refine review [--apply]` | [references/harness.md](references/harness.md) |
| Undo something you remembered | `pa refine list` / `rollback <id>` | [references/harness.md](references/harness.md) |
| Reattach to a session | `pa attach [name]` / `pa log -f` | [references/continuity.md](references/continuity.md) |
| Keep an objective across turns | `pa goal set` / `status` / `complete` | [references/continuity.md](references/continuity.md) |
| Check in on a timer | `pa heartbeat set` / `add` / `list` | [references/continuity.md](references/continuity.md) |
| Run a prompt later or on cron | `pa schedule add` / `list` | [references/continuity.md](references/continuity.md) |
| Work until the tests pass | `pa autonomous on --gate` / `step` | [references/continuity.md](references/continuity.md) |
| Deliver everything that is due | `pa tick` | [references/continuity.md](references/continuity.md) |
| Shrink a long session | `pa compact status` / `run` | `pa compact --help` |
| See installed skills | `pa skills list` / `prompt` | reads Claude, opencode and `.agents` dirs |
| Get the supplemental prompt | `pa prompt` | harness + skills + live goal, one block |

## Setup

```bash
cd ~/.agents/skills/umoja && ./install.sh
```

Builds the release binary and links it into `~/.local/bin/pa`. Run `pa status`
to confirm; it reports which harness will run agent turns.

## Three rules the tool enforces, and why

**Evidence before memory.** `pa harness remember` refuses an entry with no
`--evidence`, and `pa refine review` drops any proposal that arrives without
one rather than inventing it. An entry nobody can justify later is an entry
nobody can safely delete, and a harness full of those is worse than an empty
one.

**Review proposes; you apply.** `pa refine review` returns candidates and
writes nothing. Applying is a separate act, and each entry applied is its own
refinement with its own rollback — so a review that got three things right and
one wrong is one command from being right.

**"Out of budget" is never reported as "done".** A goal that exhausts its token
budget ends as `budget-exhausted`, and `pa goal complete` then fails. Only
finishing the work marks it complete. The same distinction runs through
autonomous mode: `finish` means the gates passed, `stop` means a limit was hit.

## Exit codes

`0` success · `1` ran fine, answer was negative (gate failed, code raised,
child agent failed) · `2` autonomous mode wants another turn · `64` bad input ·
`65` state forbids it · `69` not installed · `70` the world failed.

So a self-driving loop is just:

```bash
while pa autonomous step; [ $? -eq 2 ]; do claude -p "$(pa prompt)"; done
```

## Notes

- Kernels start lazily and exit after an hour idle. `pa kernel stop` ends one now.
- `PA_RUNNER=claude|opencode|dry-run` picks the harness; `dry-run` reports what
  would happen without spending anything.
- `PA_KERNEL=python|node|shell` (or `--lang`) picks the namespace. The Node
  kernel carries `rlm`; the file toolkit is Python-only. The shell kernel
  persists `cd` and `export` only — it holds no objects, and says so.
- Sessions are not owned by a terminal: work runs detached and state is on
  disk, so `pa attach` can be run from anywhere, at any time, from several
  terminals at once.
- Full architecture and the reasoning behind each boundary:
  [references/architecture.md](references/architecture.md).
