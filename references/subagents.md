# Subagents and messaging

Prime Agent's `rlm(...)`: real child agents, admitted and then left to work.

## Spawning returns a handle, not an answer

```bash
pa agent spawn "Review the public API for breaking changes" --name api-reviewer
pa agent spawn "Review the test coverage" --name test-reviewer
pa agent spawn "Run the slow integration audit" --name integration-audit
```

Each call returns the instant the child is admitted:

```text
admitted api-reviewer on claude-sonnet-5 (depth 1)
it will not reply here; read `pa inbox` or its files
```

This is the feature, not a limitation. A parent that blocked on its children
could only ever fan out one level and would spend the whole run idle. Spawn
what you need, **end your turn**, and read the replies later.

Results come back two ways:

- the child runs `pa send <parent> "…" --role parent`, and you read `pa inbox`;
- or the child writes files and you read them.

## Depth

The default maximum depth is 1: a root session may create children, and those
children may not create grandchildren. Raise it with `PA_MAX_DEPTH=2`. The
check happens **before** anything is created, so a refusal leaves no orphan
session behind.

## Models

A child inherits the parent's model and harness unless told otherwise:

```bash
pa agent spawn "Second opinion" --name skeptic --model opus --with opencode
```

There is deliberately no fallback to "some other model that happens to be
available". If the requested model cannot be used, the spawn fails — silently
answering with a different mind than the one asked for is worse than an error.

## Watching and settling

```bash
pa agent list                 # this session's children, oldest first
pa agent settle api-reviewer --status completed --input-tokens 900 --output-tokens 300
pa agent delete api-reviewer  # stop addressing it; the transcript stays on disk
```

`settle` folds the child's cost into the parent **exactly once** — settling
twice does not double-charge, so replaying a registry after a restart is safe.
`pa agents --json` reports `total_tokens` and `own_tokens` separately, so a
parent's own consumption stays visible underneath its children's.

"Delete" means stop addressing, never destroy evidence. A completed child stays
addressable until you delete it.

## Messaging

```bash
pa send api-reviewer "Recheck the endpoint after the latest edit" --role child
pa send all "Wrap up and report" --mode follow-up
pa inbox                # what is waiting
pa inbox --consume      # read it and clear the queue
pa roster               # everyone this session may address
```

Delivery depends on what the target is doing, and the receipt says which
happened:

| Mode | Idle target | Busy target |
|---|---|---|
| `auto` (default) | `delivered` | `queued` |
| `steer` | `delivered` | `delivered` — interrupts on purpose |
| `follow-up` | `queued` | `queued` — always waits its turn |

`pa send all` reaches the sender's **family** — parent, children, siblings —
and stops there. Being able to shout at every session on the machine, including
somebody else's unrelated work, is a footgun rather than a feature.

Consuming is destructive on purpose: a message that stayed pending after being
read would be delivered again on the next turn.
