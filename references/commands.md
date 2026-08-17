# Full command reference

Every command takes these global flags:

| Flag | Meaning |
|---|---|
| `--json` | machine-readable output |
| `-s, --session <sel>` | act on this session (name or id); defaults to `$PA_SESSION`, then one named after the working directory |
| `-C, --workdir <dir>` | working directory for runs, gates and skill discovery |
| `--runner <name>` | `claude`, `opencode` or `dry-run` |
| `--home <dir>` | state directory; defaults to `$PRIME_AGENT_HOME`, then `~/.prime/agent` |

## Sessions

```bash
pa start [name] [--model M]     # idempotent; re-running is a no-op
pa agents [--all]               # live first, newest first
pa rename <sel> <name>
pa stop <sel> [--force]         # also stops that session's kernels
pa status                       # home, runner, counts, what fires next
pa doctor [--fix]               # find inconsistent state; --fix reconciles it
pa shutdown [--force]           # stop every session and kernel
pa log [-n 40]                  # the transcript
```

Sessions are auto-created on first use, so no command needs a setup step first.

## Kernel

```bash
pa kernel exec 'code' [--timeout 120] [--max-output 16384] [--lang python|node|shell]
pa kernel exec -                # program on stdin
pa kernel vars | status | reset | stop | snapshot | restore
```

## Harness and refinement

```bash
pa harness remember <name> <body> --evidence E [--outcome O]
                     [--kind memory|prompt-note|skill|subagent]
                     [--scope local|global] [--tags a,b]
pa harness list [--kind K] | show <name> | prompt
pa harness forget <name> --evidence E [--scope S]

pa refine list [-n 20] [--global] | show <id> | rollback <id>
```

## Subagents and messaging

```bash
pa agent spawn <prompt> [--name N] [--model M] [--with claude|opencode] [--system-prompt S]
pa agent list [--all]
pa agent settle <sel> [--status completed|failed|cancelled] [--input-tokens N] [--output-tokens N]
pa agent delete <sel>

pa send <target> <message> [--role parent|child|sibling|peer|all] [--mode auto|steer|follow-up]
pa send all <message>
pa inbox [--consume]
pa roster
```

## Continuity

```bash
pa goal set <objective> [--budget N] [--deadline 2h] [--replace]
pa goal status | pause | resume | complete | clear

pa heartbeat set <prompt> [--every 10m] [--mode M]      # the user's single heartbeat
pa heartbeat add <prompt> [--every 10m] [--label L]     # an agent-owned one
pa heartbeat list [--all] | pause <id> | resume <id> | remove <id> | clear

pa schedule add <target> <when> <prompt> [--mode M]
pa schedule list [--all] [--target T] | cancel <id>

pa autonomous on [--gate CMD]… [--max-continuations N] [--max-turns N]
                 [--max-tokens N] [--max-time 1h]
pa autonomous status | step | off

pa compact status | run [--instruction "keep X"]

pa tick [--dry-run]
```

## Skills and prompt

```bash
pa skills list | show <name> | prompt
pa prompt                       # harness + skills + live goal, one block
```

Skill discovery reads, in precedence order: `--skill` paths, then project
`.prime/agent/skills`, `.agents/skills` and `.claude/skills` (walking up to the
repository root), then the same three under `$HOME`, then built-ins. A
higher-precedence skill silently overrides a lower one; two in the same scope
keep the first and report the collision.

## Environment

| Variable | Effect |
|---|---|
| `PRIME_AGENT_HOME` | state directory |
| `PA_SESSION` | default session selector |
| `PA_RUNNER` | default harness |
| `PA_MODEL` | model for new sessions |
| `PA_MAX_DEPTH` | subagent recursion limit (default 1) |
| `PA_KERNEL` | default kernel language |
| `PA_KERNEL_PYTHON` / `PA_KERNEL_NODE` / `PA_KERNEL_SHELL` | interpreter to run |
| `PA_CLAUDE_BIN` / `PA_OPENCODE_BIN` | harness binaries |
| `PA_BIN_DIR` | where `install.sh` links `pa` |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | ran fine, the answer was negative (gate failed, code raised, delivery failed) |
| `2` | autonomous mode wants another turn |
| `64` | bad input |
| `65` | the state forbids it (completing an exhausted goal, name collision) |
| `69` | not installed / unsupported |
| `70` | the world failed (disk, process, interpreter) |
