# The kernel — prompt-as-a-variable

A long-lived interpreter process, one per session per language, reached over a
Unix socket. `pa` itself is a one-shot binary; the namespace is not inside it,
which is exactly why a variable survives from one tool call to the next.

## The file toolkit

Bound in every Python kernel, no import needed, and hidden from `pa kernel
vars` so your own bindings stay legible.

```bash
pa kernel exec 'load("crates/**/*.rs")'      # 57 files, 511.6 KB in FILES
pa kernel exec 'for h in grep("pub fn call"): print(h)'
pa kernel exec 'print("\n".join(outline("src/lib.rs")))'
```

| Call | Returns |
|---|---|
| `load(pattern, root=".")` | Reads a glob into `FILES`. Prints a tally, never contents. |
| `grep(pattern, where=None, context=0, limit=200, ignore_case=False)` | `path:line: text` matches. |
| `outline(path)` | A file's definitions — its shape, not its text. |
| `head(path, lines=40)` | The first N lines. |
| `slice_lines(path, start, end)` | Lines `start`..`end`, 1-indexed, inclusive. |
| `write(path, text)` | Writes, and refreshes `FILES`. |
| `edit(path, old, new, count=1)` | Exact replacement; refuses when ambiguous. |
| `sh(command, cwd=None)` | `(exit_code, stdout, stderr)`. |

### Why these exist rather than being left to the caller

A kernel only saves anything if the reduction happens *here*. Loading a hundred
files and then printing them back out costs exactly what reading them one at a
time would have cost — the namespace added a step and saved nothing.

That mistake is easy precisely because `print(FILES[path])` is one short line
while writing a grep by hand is five. So the short line is the reducing one:
`load` never prints a file, `grep` returns matches rather than contents, and
`outline` returns shape rather than text. Reaching for a whole file is still
possible and still sometimes right — it is just no longer the path of least
resistance.

`FILES` survives `pa kernel reset` as a name but not as data: the toolkit is
plumbing, its contents are yours, and "empty the namespace" has to mean it.

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
