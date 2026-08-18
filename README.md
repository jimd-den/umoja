# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents

*Umoja* — Swahili for "unity," the first principle of Kwanzaa: working as one
toward a shared purpose. This is [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)'s
capabilities as one Rust binary (`pa`), usable from Claude Code, opencode, or
a plain shell.

```bash
./install.sh
pa kernel exec 'rows = [json.loads(l) for l in open("huge.jsonl")]'
pa kernel exec 'len(rows)'      # a separate process; `rows` is still there
```

- **A persistent kernel** — Python, Node or shell. Variables survive between
  tool calls, so large data is loaded once and queried many times instead of
  being printed into the conversation.
- **A file toolkit in the namespace** — `load`, `grep`, `outline`, `head`,
  `slice_lines`, `write`, `edit`, `sh`. Searching a tree returns the four
  matching lines rather than the fifty-seven files they were found in.
- **A continual harness** — evidence-backed memories, notes, skill and subagent
  specs, with a before/after paper trail and one-command rollback.
  `pa refine review` reads a session's own trajectory back and proposes what is
  worth keeping, dropping anything it cannot justify.
- **Subagents** — `rlm(...)` inside the kernel asks a child a question and
  returns its answer as a string; `pa agent spawn` admits one and leaves it to
  work, replies arriving as messages. Many agents pulling in one direction —
  *umoja*.
- **Reattachable sessions** — no terminal owns a session, so `pa attach` works
  from anywhere, at any time, from several terminals at once.
- **Goals, heartbeats, schedules, autonomous gates, compaction** — work that
  outlives the turn, with budgets that are never reported as completion.

Start at [SKILL.md](SKILL.md). Design notes in
[references/architecture.md](references/architecture.md).

MIT.
