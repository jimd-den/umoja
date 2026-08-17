# The kernel — prompt-as-a-variable

A long-lived interpreter process, one per session per language, reached over a
Unix socket. `pa` itself is a one-shot binary; the namespace is not inside it,
which is exactly why a variable survives from one tool call to the next.

## The discipline

1. **Load once, in the kernel.** Never pipe a large file through the
   conversation to get it into a variable.
2. **Reduce, then print.** Filter, count, aggregate, slice — and print the
   number, the row, or the short list that is the actual answer.
3. **Never print the big thing.** If you are unsure of a size, `pa kernel vars`
   tells you without showing you.

```bash
pa kernel exec 'import json; rows = [json.loads(l) for l in open("events.jsonl")]'
pa kernel exec 'len(rows)'
pa kernel exec 'sorted({r["service"] for r in rows})[:10]'
pa kernel exec 'sum(r["ms"] for r in rows) / len(rows)'
```

## Commands

```bash
pa kernel exec 'code'          # run; the last expression prints, like a REPL
pa kernel exec -               # read the program from stdin (heredocs, generated code)
pa kernel exec 'code' --timeout 600
pa kernel vars                 # names, types, lengths, sizes — never values
pa kernel status               # cold | ready | dead
pa kernel reset                # empty the namespace, keep the process
pa kernel stop                 # end the process
pa kernel snapshot             # pickle the namespace to disk
pa kernel restore              # load it back into a fresh kernel
```

Add `--lang node` or `--lang shell` to use a different namespace. Each language
gets its own, so switching does not disturb the other.

## What happens on failure

An exception is an *outcome*, not an error: you get the traceback, exit code 1,
and **the namespace is untouched**. The expensive thing you loaded ten minutes
ago is still there.

```bash
pa kernel exec '1/0'          # ZeroDivisionError, exit 1
pa kernel exec 'len(rows)'    # still 4213908
```

A runaway loop hits `--timeout` (120s default), is interrupted, and again
leaves the namespace alive. `sys.exit()` inside model-generated code is ignored
rather than being allowed to kill the kernel.

## Output is clipped, never silently

Output beyond `--max-output` (16KB default) is clipped from the middle, keeping
the head and the tail, with a note saying how many bytes went. A traceback's
first and last lines carry the meaning; the middle of a dump rarely does.

## Snapshots

`pa kernel snapshot` pickles everything picklable and **reports what it could
not take** rather than failing. Most namespaces hold one open file handle and a
hundred perfectly serialisable results; a partial snapshot that is honest about
the gap beats a refusal.

## Choosing a language

| Language | Holds objects | Use for |
|---|---|---|
| `python` (default) | yes | anything data-shaped |
| `node` | yes | when the project's parsing is already JavaScript |
| `shell` | **no** | a working directory and exported variables, nothing more |

The shell kernel is honest about its limits: `pa kernel vars --lang shell`
reports the directory and the variables *you* exported, not the ambient
environment.

## Environment

- `PA_KERNEL` — default language.
- `PA_KERNEL_PYTHON` / `PA_KERNEL_NODE` — the interpreter to run, so a project
  can point at its own virtualenv.
- Kernels exit after an hour idle and restart on next use.

## When *not* to use it

If the answer is one `grep` away, use `grep`. The kernel earns its keep when
the same data is queried more than once, or when the reduction takes more than
one step.
