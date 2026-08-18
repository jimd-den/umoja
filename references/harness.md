# The continual harness

Durable, supplemental state that a session accretes: prompt notes, memories,
descriptions of reusable calls, and subagent specs. Prime Agent's rule holds
here too — **the base prompt is immutable**. Everything the harness holds is
additional, and every change to it is reversible.

## Writing something down

```bash
pa harness remember prefers-rust "The user wants memory-safe languages." \
  --evidence "Asked for Rust or Go explicitly, twice." \
  --outcome  "Stop proposing Python for new tools; check by re-reading the next request."
```

`--evidence` is **required**. This is the whole discipline: an entry nobody can
justify later is an entry nobody can safely delete, and a harness full of
unjustifiable entries is worse than an empty one.

Before writing, ask the four questions Prime Agent's `/refine` planner asks:

1. **Is there real evidence?** A correction the user had to give, a failure that
   would recur, an explicit preference. One unusual event is not a pattern.
2. **What is the smallest artifact?** `--kind memory` for a fact,
   `--kind prompt-note` for a behavioural rule, `--kind skill` for a reusable
   call's contract, `--kind subagent` for a delegation role.
3. **What should improve, and how would you check?** That is `--outcome`, and
   it is what makes a later rollback an informed decision instead of a guess.
4. **Local or global?** `--scope local` (default) means this project.
   `--scope global` means a cross-project lesson about the user or about how you
   should generally work — rare, and worth a second thought.

## Reading it back

```bash
pa harness list                  # everything visible here: local + global
pa harness list --kind memory
pa harness show prefers-rust     # full body, evidence and outcome
pa harness prompt                # the block to splice into a prompt
```

`pa harness prompt` emits **headlines only** — one line per entry. A hundred
memories cost a hundred lines, not a hundred paragraphs. `pa prompt` emits the
harness, the installed skills and any live goal as a single block.

## Asking what is worth remembering

```bash
pa refine review                 # propose, write nothing
pa refine review --apply         # write them, one refinement each
pa refine review --raw           # the reviewer's reply, verbatim
pa refine review -n 100          # look further back
```

This reads the session's recent trajectory and its existing entries, and asks
the session's own harness the four questions above. It **proposes only** unless
`--apply` is given.

That separation is the whole design. The evidence rule only bites if something
stands between noticing a lesson and recording one; a review that wrote
straight to the harness would fill it with confident generalisations drawn from
single events, each carrying a plausible sentence in its evidence field, and
the harness would be worse than empty because every entry would look justified.

Two things follow from that:

- **A proposal with no evidence is dropped, never repaired.** Inventing the
  justification is precisely the thing that must not happen.
- **Proposing nothing is a good answer, and the common one.** An empty
  trajectory is not even sent to a model — asking one to review nothing
  reliably produces invented lessons.

Applying writes one refinement per proposal, so a review that got three things
right and one wrong is one `pa refine rollback` from being right.

## Undoing

Every write records a before/after snapshot, so rolling back is mechanical
rather than a second guess at what the state used to be.

```bash
pa refine list                 # newest first, with applied/reverted state
pa refine show <id>            # the before and the after, side by side
pa refine rollback <id>        # undo it
```

- Rolling back a **create** removes the entry.
- Rolling back an **update** restores the previous body.
- Rolling back a **delete** puts the entry back.

The rollback is itself recorded, and the original is stamped with the id that
reverted it. Nothing is erased — the history of what was tried is the only
reason any of this is trustworthy. A refinement cannot be rolled back twice.

## Removing an entry

```bash
pa harness forget prefers-rust --evidence "They moved the project to Go."
```

Deletion is reversible for the same reason everything else is.

## Where it lives

```text
~/.prime/agent/harness/harness_state.json                 global entries
~/.prime/agent/harness/refinements.jsonl                  global paper trail
~/.prime/agent/session-artifacts/<id>/harness/…           local entries
~/.prime/agent/session-artifacts/<id>/refinements.jsonl   local paper trail
```

Refinement logs are append-only. "Reverted" is recorded as a later line, never
by editing an earlier one.
