# Work that outlives the turn

Goals, heartbeats, schedules, autonomous mode and the tick that drives them.

## Goals

A durable objective the harness keeps presenting until it is done.

```bash
pa goal set "Ship the release and verify every published artifact" --budget 200000
pa goal set "Migrate the repo" --deadline 2h --replace
pa goal status
pa goal pause / resume / clear
pa goal complete
```

**Only `pa goal complete` marks success.** A goal that exhausts its budget ends
as `budget-exhausted` and can no longer be completed — the distinction between
"finished" and "ran out of money" is preserved all the way to the exit code,
because a harness that reports the second as the first is worse than one that
reports nothing.

Creating a goal is an explicit act. Nothing infers one from a task: a tool that
quietly decides it now has a long-running objective is a tool that keeps
spending after you thought it stopped.

## Heartbeats

Recurring instructions that re-enter a session.

```bash
pa heartbeat set "Check the deployment and report meaningful changes" --every 10m
pa heartbeat add "poll the CI run" --every 2m --label ci --mode follow-up
pa heartbeat list
pa heartbeat pause <id> / resume <id> / remove <id>
pa heartbeat clear
```

`set` maintains the **user's single visible heartbeat** and replaces any
previous one. `add` creates an agent-owned heartbeat alongside the others, and
an agent can never modify or clear the user's — an agent that could silence the
instruction telling it to check in is an agent that can stop reporting.

Intervals: `30s`, `10m`, `1h30m`, `2d`. A bare number means minutes.

A heartbeat that was missed for an hour fires **once**, then schedules from now.
One missed check-in is a smaller failure than a backlog of twelve.

## Schedules

One-time or cron prompts aimed at any agent.

```bash
pa schedule add worker "in 30m" "Check the benchmark result"
pa schedule add worker "0 9 * * 1-5" "Review open work"
pa schedule add worker "2026-09-01T09:00:00Z" "Start the quarterly audit"
pa schedule list --all
pa schedule cancel <id>
```

Cron is the familiar five fields, with names (`mon`, `jan`), ranges, lists and
steps. A bare interval like `30m` is **refused** rather than guessed at —
"in 30m" and "every 30m" are different requests.

A due tick is *claimed* before it is delivered, so a crash mid-delivery does not
replay an uncertain prompt. A failed delivery retries a cron job on its next
tick and retires a one-time job.

## Autonomous mode

Bounded continuation for runs where nobody is watching.

```bash
pa autonomous on --gate "cargo test" --gate "cargo clippy -- -D warnings" \
  --max-turns 20 --max-tokens 500000 --max-time 1h
pa autonomous step      # run the gates, decide
pa autonomous status
pa autonomous off
```

`step` exits **2** when it wants another turn, so a loop is one line:

```bash
while pa autonomous step; [ $? -eq 2 ]; do claude -p "$(pa prompt)"; done
```

Three things worth knowing:

- **A failing gate's own output comes back verbatim** in the continuation
  prompt. Paraphrasing a compiler error helps nobody.
- **A failed gate is not re-run against an unchanged workspace.** The workspace
  is fingerprinted (git status where available, file sizes and nanosecond mtimes
  otherwise); an identical fingerprint means an identical result, so the run is
  skipped and reported as skipped.
- **Limits are checked before gates.** A run that is out of budget says so
  rather than spending more of it running a test suite. `finish` means the gates
  passed; `stop` means a limit was hit. They are never conflated.

## The tick

One pass that delivers everything due: heartbeats, schedules, goal
continuations.

```bash
pa tick --dry-run    # what would be delivered, without running anything
pa tick              # deliver it
```

No sleeping and no threads, so it is safe from cron, from a loop, or by hand:

```cron
* * * * * PA_RUNNER=claude /home/you/.local/bin/pa tick >> ~/.prime/agent/tick.log 2>&1
```

Ticks exit non-zero when a delivery failed, so a wrapper notices. When
autonomous mode is on for a session, it has the final say over that session's
goal continuations: the goal is the objective, the policy decides whether there
is budget left to pursue it.

## Compaction

```bash
pa compact status
pa compact run --instruction "Keep the failing tests and the remaining migration steps"
```

Compaction summarises older events and keeps recent ones. It is **not** a
completion signal: goals, heartbeats, children and — importantly — the kernel
namespace all survive it untouched. That last point is the practical argument
for loading data into a variable rather than printing it: compaction can take
the transcript, but it cannot take `rows`.
