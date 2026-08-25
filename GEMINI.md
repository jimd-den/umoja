# Antigravity (AGY) & UMOJA Rules

Every file operation goes through `umoja kernel exec`. Never use native
`view_file` / `Read`, `grep_search`, `write_to_file` / `Write`, or
`replace_file_content` / `Edit`.

## 1. Only guarded editors may touch code

An unguarded edit writes whatever you hand it. If the patch does not compile,
the file on disk is now broken and every later read sees corrupted source.
This is the most common way an agent destroys a working tree.

**Use these. They write, check, and roll back on failure:**

| Guarded | Purpose |
|---|---|
| `try_replace_lines(path, start, end, text)` | line range, validated |
| `try_edit(path, old, new)` | exact substring, validated |
| `try_replace_fn(path, fn_name, body)` | whole function by name, validated |
| `try_add_to_mod(path, mod_name, code)` | insert inside named module before closing brace, validated |
| `ast_rewrite(pattern, rewrite, path[, lang])` | structural rewrite, validated |
| `create_module(path, code[, parent_mod])` | new file, linked into the module tree, validated |

Each returns `{ ok, errors, checker, guarded }`. `errors` is always present.
Check `ok` before doing anything else.

**Never reach for `replace_lines`, `edit`, `write`, `insert_at_line`,
`insert_after`, `replace_fn`, `replace_struct`, `write_b64` or
`replace_lines_b64` as an alternative.** They check nothing.

### The one legitimate use of an unguarded edit

A change that is only valid **as a set** — an enum variant plus its match arm,
or moving a `pub mod` line — has an intermediate state that cannot compile, so
each guarded edit is rejected on its own. Only then:

```rhai
edit(path, old_a, new_a);
edit(path, old_b, new_b);
let chk = lsp_check(path);        // MANDATORY, never omit
if !chk.ok { for e in chk.errors { print(e.message); } }
```

An unguarded edit with no following `lsp_check` is a defect.

## 2. Know whether you are actually guarded

`try_*` keeps an edit when nothing objects — including when **no checker
exists** for that file type. A `.ts`, `.go` or `.ats` file is written
unverified and `ok: true` means only "nothing spoke".

- `lsp_available(path)` → `{ checker, guarded, note }`. Ask **before** editing.
- Checkers: `cargo`, `rustc`, `python3`, `json`. Everything else is `none`.
- `capabilities()` → `cargo`, `rustc`, `ast-grep`, `python3`, `git`, each with
  `installed` / `version` / `install_hint`, plus a `missing` list.

Unverifiable file type? Prefer `ast_rewrite`, then run the project's tests.

## 3. Search structurally first

`grep` matches inside strings and comments and misses wrapped calls.
`ast_grep` matches the parse tree.

```rhai
let hits = ast_grep("fn $NAME($$$ARGS) -> Result<$T> { $$$BODY }", "crates/**/*.rs", "rust");
for m in hits.matches { print(`${m.file}:${m.line}`); }
```

`$X` is one node, `$$$X` is many. Lines are 1-indexed.

If `ast_grep_available().installed` is false, print its `install_hint` and
**ask the user** before installing anything. Use plain `grep` for prose,
config, logs, and languages with no grammar.

## 4. Log what you did and why

```rhai
log_action("split walk.rs into checking/expr.rs", "crates/application",
           "walk.rs mixed traversal with type rules, so neither could be tested alone");
```

A diff preserves what changed and destroys why. `log_action` **refuses a line
with no reason**. Read it back with `actions()`.

## 5. Report tool defects rather than working around them

```rhai
report_bug("component", "one-line title", "Expected / Observed / Repro");
report_error("component", "title", "body");
report("friction", "component", "title", "body");
reports_markdown();     // ready to paste into an issue
```

A body must say what was expected, what happened, and how to reproduce it, or
it is refused. Reports stay local in `~/.umoja/reports.jsonl`; filing one
never sends anything anywhere.

## 6. What is recorded whether you ask or not

`log_action` and `report_bug` need you to call them. Two things do not:
**every `umoja` run** and **every file mutation** are written to SQLite at
`<project>/.umoja/activity.db` automatically, including whether a checker was
in the loop. The journal is per-project and ignores itself, so it never shows
up in `git status`.

```bash
umoja activity              # recent commands
umoja activity --changes    # recent mutations; unverified writes flagged UNVERIFIED
```

After 5 changes with no report filed, any `umoja` command prints a reminder
to stderr naming the call to make. It asks once per batch; filing a report
resets it. Do not ignore a defect you actually worked around.


## 7. Use the rest of the system

```bash
umoja goal set/add/check/status      # checklist in SQLite, one line in context
umoja send --to parent "..."         # message bus
umoja inbox [--consume]              # check at the start of a turn in a fleet
umoja harness remember/search/rollback
umoja agent call <name> "..."        # subagents
umoja heartbeat/schedule/tick
umoja compact                        # fold the transcript when context tightens
```

## Rhai in one screen

- `let x = 10;` `const PI = 3.14;` — `new` is a reserved word.
- Strings: `"text"`, `` `val: ${x}` ``, raw `r#"..."#`. Use `r###"..."###`
  when the payload itself contains `r#"`, or the literal ends early.
- Maps: `#{ name: "Alice", score: 95 }`; arrays: `[1,2,3]`, `.push`, `.len`.
- Lambdas: `|x| x.score > 80`. Loops: `for item in list { }`.
- A missing map key reads as `()` and is **not iterable** — `for e in res.errors`
  throws when `errors` is absent. Guarded results always include it.

## Reading without drowning

`outline(path)` for shape, `slice_lines(path, a, b)` for a region,
`head(path, n)`, `load(glob)` to index, `grep(pattern, glob)` for text.
Reduce in the kernel with `sum_by`, `group_by`, `count_by`, `sort_by`,
`filter_eq`, `pluck`, `unique_by` — print the answer, never the data.

## Red-green TDD

Write the failing test first with `try_replace_lines`, run it via
`sh("cargo test ...")`, confirm it fails, then make it pass with `try_edit`,
then `cargo test --workspace`, then `log_action` and
`umoja harness remember`. The failing test is the only evidence the change
did anything.
