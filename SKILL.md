---
name: umoja
description: UMOJA (Umoja Manages Orchestrated Joint Agents) — MANDATORY FILE & KERNEL ENGINE FOR ANTIGRAVITY (AGY) & CLAUDE CODE. Always activate this skill for ANY task involving reading, inspecting, searching, editing, or creating files, running tests, exploring codebases, or handling multi-turn context. The agent MUST NOT use native view_file, grep_search, or edit tools to browse or edit code; instead, use `umoja kernel exec` (with Rhai functions `head`, `outline`, `slice_lines`, `load`, `grep`, `edit`, `write`), `umoja agent call` (subagents/RLM), `umoja harness remember/list/review/rollback`, persistent goals, heartbeats, and SQLite memory with FTS5 search.
license: MIT
compatibility: Linux or macOS. Rust toolchain to build. Pure Rust (Rhai) in-process kernel, embedded SQLite database. No Python or external socket daemon needed. No network access required.
---

# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents. *Umoja* is Swahili
for "unity" — one namespace, many agents, pulling in one direction.

## MANDATORY RULE: The kernel is how you touch files

This is the central discipline of UMOJA and is **strictly enforced** across all harnesses (Antigravity/AGY, Claude Code, opencode, plain shell):

| Instead of (DO NOT USE) | Use (MANDATORY) |
|---|---|
| Native `view_file` / `Read` | `umoja kernel exec 'print(head("path"))'`, or `outline`, or `slice_lines` |
| Native `read_file` / multi-read | `umoja kernel exec 'let code = load("src/**/*.rs");'` once, then query |
| Native `grep_search` / `find_by_name` | `umoja kernel exec 'grep("pattern")'` over loaded files |
| Native `write_to_file` / `Write` | `umoja kernel exec 'write("path", text)'` |
| Native `replace_file_content` / `Edit` | `umoja kernel exec 'edit("path", old, new)'` |
| Native command execution for data exploration | `umoja kernel exec 'sh("cmd")'`, so output lands in a variable |

**Why the rule is exclusive rather than advisory.** Reading a file with native view/read
puts its entire text in the conversation permanently, consuming enormous token budgets.
`umoja` loads files into the persistent pure Rust embedded kernel, keeping raw text out of context,
and returns only targeted slices, outlines, or summaries.

## The one idea worth internalising

**Load data into a variable; print only the answer.**

Reading a 200MB log or 50 source files into the conversation costs tokens proportional to its
size. Loading it into the pure Rust kernel and printing `len(errors)` costs eleven tokens. The
pure Rust kernel persists variables across tool calls and sessions — so the variable is still there on your
next tool call — and the one after that.

```bash
umoja kernel exec 'let rows = [#{id: 1, status: "error"}, #{id: 2, status: "ok"}];'   # nothing printed
umoja kernel exec 'rows.len()'                                                         # 2
umoja kernel exec 'let errors = rows.filter(|r| r.status == "error"); errors.len()'   # 1
```

Three calls, one load, and the conversation never sees a single raw row. Use
`umoja kernel vars` to see what is bound — names, types and sizes, never values.

### The failure mode this skill is designed around

Loading a hundred files and then printing them back out costs exactly what
reading them one at a time would have cost. The namespace added a step and
saved nothing. This is *the* easy mistake, and it is easy because
`print(FILES[path])` is one short line.

So the short lines are the reducing ones. Before any call that prints, ask:
**am I about to print something I could search, count, or slice first?**

### Climb the ladder before you print

Measured on eight Dart files a real task actually needed: the declarations
were **6% of the source bytes**. The other 94% was scrolled past. Reach for
the cheapest rung that answers the question, and only fall to the next when
it genuinely does not:

1. `grep(...)` — you know roughly what you are looking for
2. `outline(path)` — you need the shape, not the bodies
3. `slice_lines(path, a, b)` — `outline` told you which lines
4. `head(path)` — the answer really is near the top
5. printing the whole text — **the last rung, and it needs a reason**

Printing a file to \"get oriented\" is the failure mode wearing a disguise.
`outline` is what orientation costs.

## The file toolkit

Bound in every Python kernel, no import needed. `pa kernel vars` hides them, so
your own bindings stay legible.

```bash
pa kernel exec 'load("crates/**/*.rs")'          # 57 files, 511.5 KB indexed
pa kernel exec 'for h in grep("pub fn call"): print(h)'
pa kernel exec 'print("\n".join(outline("src/lib.rs")))'
```

| Call | Returns |
|---|---|
| `load(pattern, root=".")` | Indexes a glob into `FILES`. Records paths, reads nothing, prints a tally. |
| `grep(pattern, where=None, context=0, limit=200, ignore_case=False)` | `path:line: text` matches — the answer, not the haystack. `where` narrows it: a glob, a list of paths, or another corpus. |
| `outline(path)` | A file's definitions, so you can decide where to look without reading the way there. |
| `head(path, lines=40)` | The first N lines, for when a peek really is enough. |
| `slice_lines(path, start, end)` | Lines `start`..`end`, 1-indexed, inclusive. |
| `write(path, text)` | Writes, and refreshes `FILES` so the next `grep` sees it. |
| `edit(path, old, new, count=1)` | Exact replacement. **Refuses** when `old` is missing or ambiguous rather than guessing. |
| `sh(command, cwd=None)` | `(exit_code, stdout, stderr)`, so a command's output stays in the namespace. |

`outline`, `head`, `slice_lines` and `edit` read any path, indexed or not —
being told "no definitions" because a path was never indexed is a wrong answer
wearing a right answer's clothes.

### Editing without reading the file

`edit(path, old, new)` needs an exact anchor, and the obvious way to get one
is to print the file and copy from it. That is how a session that indexed two
hundred files still pays full price for eight of them — the single most
expensive habit this skill exists to prevent, and the toolkit table above
does not stop it.

**Grep for the anchor instead. The match is the anchor.**

```bash
pa kernel exec 'for h in grep(r"required this.programmable", context=3): print(h)'
pa kernel exec 'edit("lib/theme.dart", "  this.seedHue,", "  this.seedHue,\n  this.tint,")'
```

`grep(..., context=N)` hands back the surrounding lines, which is exactly the
unique anchor `edit` wants, at the cost of six lines rather than six hundred.
And `edit` refuses an ambiguous or missing match instead of guessing, so too
short an anchor fails loudly and you widen the context. That refusal is what
makes it safe never to see the rest of the file.

The exception is a file you are *rewriting* rather than amending. Then read
it — you are about to be responsible for all of it.

### The corpus is an index, not a cache

`load` records paths and reads nothing; every access reads the file as it is on
disk right now. Measured on 14,293 files (338MB of Rust):

| | load | search | kernel RSS |
|---|---|---|---|
| holding the text | 0.89s | 0.48s | **391MB** |
| indexing the paths | **0.32s** | 0.84s | **25MB** |

Twice the search time for a sixteenth of the memory, and on a normal project
(63 files) the two are indistinguishable at 0.01s — so the trade is paid only
where it is affordable. A bounded cache was tried and lost to both: 73MB bought
no speed at all.

The deciding argument is not memory. A cache is a second copy of the truth, and
it goes stale the moment anything edits a file behind it — another process, a
git checkout, the harness's own editor. Reading on demand cannot be wrong about
what is on disk.

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
| Keep data out of context | `umoja kernel exec` / `vars` / `reset` | [references/kernel.md](references/kernel.md) |
| Delegate and wait | `umoja agent call` — or `rlm(...)` in the kernel | [references/subagents.md](references/subagents.md) |
| Delegate and carry on | `umoja agent spawn` / `list` / `settle` | [references/subagents.md](references/subagents.md) |
| Talk between agents | `umoja send` / `umoja inbox` / `umoja roster` | [references/subagents.md](references/subagents.md) |
| Remember something durably | `umoja harness remember` / `list` | [references/harness.md](references/harness.md) |
| Ask what is worth remembering | `umoja refine review [--apply]` | [references/harness.md](references/harness.md) |
| Undo something you remembered | `umoja refine list` / `rollback <id>` | [references/harness.md](references/harness.md) |
| Reattach to a session | `umoja attach [name]` / `umoja log -f` | [references/continuity.md](references/continuity.md) |
| Keep an objective across turns | `umoja goal set` / `status` / `complete` | [references/continuity.md](references/continuity.md) |
| Check in on a timer | `umoja heartbeat set` / `add` / `list` | [references/continuity.md](references/continuity.md) |
| Run a prompt later or on cron | `umoja schedule add` / `list` | [references/continuity.md](references/continuity.md) |
| Work until the tests pass | `umoja autonomous on --gate` / `step` | [references/continuity.md](references/continuity.md) |
| Deliver everything that is due | `umoja tick` | [references/continuity.md](references/continuity.md) |
| Shrink a long session | `umoja compact status` / `run` | `umoja compact --help` |
| See installed skills | `umoja skills list` / `prompt` | reads Claude, opencode, AGY, and `.agents` dirs |
| Get the supplemental prompt | `umoja prompt` | harness + skills + live goal, one block |

## Setup

```bash
cd ~/.agents/skills/umoja && ./install.sh
```

Builds the release binary and links it into `~/.local/bin/umoja` (and `~/.local/bin/pa`). Run `umoja status`
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
