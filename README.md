# prime-agent

[Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)'s capabilities
as one Rust binary, usable from Claude Code, opencode, or a plain shell.

```bash
./install.sh
pa kernel exec 'rows = [json.loads(l) for l in open("huge.jsonl")]'
pa kernel exec 'len(rows)'      # a separate process; `rows` is still there
```

- **A persistent kernel** — Python, Node or shell. Variables survive between
  tool calls, so large data is loaded once and queried many times instead of
  being printed into the conversation.
- **A continual harness** — evidence-backed memories, notes, skill and subagent
  specs, with a before/after paper trail and one-command rollback.
- **Subagents** — admitted and left to work; replies arrive as messages.
- **Goals, heartbeats, schedules, autonomous gates, compaction** — work that
  outlives the turn, with budgets that are never reported as completion.

Start at [SKILL.md](SKILL.md). Design notes in
[references/architecture.md](references/architecture.md).

MIT.
