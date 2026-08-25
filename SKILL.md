---
name: umoja
description: UMOJA (Umoja Manages Orchestrated Joint Agents) — MANDATORY FILE & KERNEL ENGINE FOR ANTIGRAVITY (AGY) & CLAUDE CODE. Always activate this skill for ANY task involving reading, inspecting, searching, editing, or creating files, running tests, exploring codebases, or handling multi-turn context. The agent MUST NOT use native view_file, grep_search, or edit tools to browse or edit code. Edits go through the LSP-guarded editors ONLY — `try_replace_lines`, `try_edit`, `try_replace_fn`, `create_module`, `ast_rewrite` — never raw `replace_lines`/`edit`/`write`, which corrupt files that fail to compile. Search structurally with `ast_grep` before falling back to text `grep`. Ask `lsp_available(path)` whether an edit will actually be verified and `capabilities()` what this machine has. Log every action with `log_action(what, why)` and file tool defects with `report_bug`. Also `umoja agent call` (subagents/RLM), `umoja harness remember/search/rollback`, `umoja goal set/add/check/status`, `umoja heartbeat/schedule/tick`, `umoja send/inbox`, `umoja compact`, and SQLite memory with FTS5 search.
license: MIT
compatibility: Linux or macOS. Rust toolchain to build. Pure Rust (Rhai) in-process kernel, embedded SQLite database. Optional but strongly recommended: `ast-grep` for structural search and rewrite. No Python or external socket daemon needed. No network access required.
---

# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents. *Umoja* is Swahili for "unity" — one persistent namespace, many agents, pulling in one direction.

---

## 🛑 RULE 1 — The kernel is how you touch files

| Instead of (DO NOT USE) | Use (MANDATORY) |
|---|---|
| Native `view_file` / `Read` | `head`, `outline`, `slice_lines`, `read` |
| Native `read_file` / multi-read | `load("src/**/*.rs")`, then query |
| Native `grep_search` / `find_by_name` | `ast_grep` first, `grep` for prose and non-code |
| Native `write_to_file` / `Write` | `create_module` (new code), `write_b64` (binary-safe) |
| Native `replace_file_content` / `Edit` | `try_replace_lines`, `try_edit`, `try_replace_fn`, `try_add_to_mod`, `ast_rewrite` |
| Native command execution for data | `sh("...")` inside the kernel |

## 🛑 RULE 2 — Only guarded editors may touch code

An unguarded edit writes whatever you gave it. If the patch does not compile,
the file on disk is now broken, and the next tool call reads corrupted source.
This is the single most common way an agent destroys a working tree.

**Guarded editors write, check, and roll back on failure. Use these:**

```bash
umoja kernel exec '
let patch = r###"
    pub fn compute(val: i32) -> i32 {
        val * 2
    }
"###;
let res = try_replace_lines("crates/infra/src/calc.rs", 10, 15, patch);
if res.ok {
    print(`applied; verified by ${res.checker}`);
} else {
    for err in res.errors { print(`  L${err.line}: ${err.message}`); }
}
'
```

| Guarded (ALWAYS PREFER) | What it does |
|---|---|
| `try_replace_lines(path, start, end, text)` | Line range, validated, rolled back on error |
| `try_edit(path, old, new)` | Exact substring, validated, rolled back on error |
| `try_replace_fn(path, fn_name, body)` | Whole function by name, validated |
| `try_add_to_mod(path, mod_name, code)` | Inserts code inside named module before closing brace, validated |
| `ast_rewrite(pattern, rewrite, path[, lang])` | Structural rewrite, validated |
| `create_module(path, code[, parent_mod])` | New file + links it into the module tree + validates |

Every one of them returns a map with **`ok`**, **`errors`** (always present, may be
empty), **`checker`**, and **`guarded`**. Read `ok` before doing anything else.

### The unguarded primitives, and the one time they are correct

`replace_lines`, `edit`, `insert_at_line`, `insert_after`, `write`,
`write_b64`, `replace_fn`, `replace_struct`, `replace_lines_b64` write
**without checking anything**. They are a last resort, not an alternative.

There is exactly one honest reason to reach for them: **a change that is only
valid as a set.** Adding an enum variant and its match arm, or moving a
`pub mod` declaration, leaves an intermediate state that cannot compile, so
each guarded edit in the pair is rejected on its own. In that case, and only
in that case:

```bash
umoja kernel exec '
edit("src/event.rs", old_variant_block, new_variant_block);
edit("src/event.rs", old_match_arm,     new_match_arm);
let chk = lsp_check("src/event.rs");      # <-- NEVER omit this
print(`compiles: ${chk.ok}`);
if !chk.ok { for e in chk.errors { print(e.message); } }
'
```

**If you use an unguarded editor you MUST call `lsp_check` before your next
action, and you MUST fix or revert what it reports.** An unguarded edit with
no following check is a defect.

## 🛑 RULE 3 — Know whether you are actually guarded

`try_*` keeps an edit when the checker raises nothing — including when there
**is no checker** for that file type. A `.ts`, `.go` or `.ats` file is written
unverified, and a naive reading of `ok: true` is wrong.

```bash
umoja kernel exec '
let g = lsp_available("src/app.ts");
print(`${g.checker} / guarded=${g.guarded}`);   # none / guarded=false
print(g.note);
'
```

Checkers that exist today: `cargo` (inside a Cargo workspace), `rustc`
(standalone `.rs`), `python3` (`.py`), `json` (`.json`). Everything else is
`none`.

- `lsp_available(path)` → `{ checker, guarded, note }` — ask **before** editing.
- Every guarded result carries `checker`/`guarded`, and a `warning` when unverified.
- `capabilities()` → what this machine has (`cargo`, `rustc`, `ast-grep`,
  `python3`, `git`), each with `installed`, `version`, `install_hint`, plus a
  `missing` list.

When a file type has no checker, prefer `ast_rewrite` (the grammar still
constrains the edit) and **run the project's own test command afterwards**.

## 🔍 RULE 4 — Search structurally before searching textually

`grep` matches inside strings, comments and unrelated identifiers, and misses
a call that wrapped onto a second line. `ast_grep` matches the parse tree.

```bash
umoja kernel exec '
let hits = ast_grep("fn $NAME($$$ARGS) -> Result<$T> { $$$BODY }", "crates/**/*.rs", "rust");
print(`${hits.count} fallible functions`);
for m in hits.matches { print(`${m.file}:${m.line}  ${m.text}`); }
'
```

Metavariables: `$X` one node, `$$$X` many nodes. Lines are 1-indexed, like
every other umoja call.

**If `ast-grep` is missing, ask the user before installing it:**

```bash
umoja kernel exec '
let a = ast_grep_available();
if !a.installed { print(a.install_hint); }   # then ASK; do not install unprompted
'
```

Use plain `grep` for prose, config, logs, and languages ast-grep has no
grammar for.

## 📝 RULE 5 — Log what you did and why

A diff records what changed and destroys the reason. Every agent logs its own
intent, at the moment it acts:

```bash
umoja kernel exec '
log_action("split walk.rs into checking/expr.rs", "crates/application",
           "walk.rs mixed traversal with type rules, so neither could be tested alone");
'
```

`log_action(action, why)` or `log_action(action, target, why)`. **A log line
with no reason is refused** — that is the whole point of the call. Read the
journal back with `actions()`.

## 🐞 RULE 6 — Report tool defects instead of working around them

When umoja itself misbehaves, file it. A workaround you do not report is a
bug the next agent rediscovers.

```bash
umoja kernel exec '
report_bug("ast_rewrite", "rewrite kept a file that does not compile",
r###"Expected: rollback on compiler error.
Observed: ok=true, file left broken.
Repro: ast_rewrite("x + 1", "x +", "/tmp/t.rs", "rust")"###);
'
```

- `report_bug([component,] title, body)` — wrong behaviour
- `report_error([component,] title, body)` — a crash or non-zero exit
- `report("friction"|"idea", component, title, body)` — a papercut, or something missing
- `reports()` / `reports_markdown()` — read them back; the markdown is ready to paste into an issue

A report **must** say what was expected, what happened, and how to reproduce
it; a body without that is refused. Reports live in `~/.umoja/reports.jsonl`
and are never sent anywhere — filing is local, and opening an issue is a
person's decision.

---

## 📊 What is recorded whether you ask or not

`log_action` and `report_bug` depend on you calling them. Two things do not:

- **Every `umoja` run** — subcommand, arguments, cwd, exit code, duration.
- **Every file mutation** — which builtin, which path, and whether a checker
  was actually in the loop.

Both land in a SQLite database at `<project>/.umoja/activity.db`, scoped to the
project they belong to — the nearest ancestor holding a `.git`, else the
working directory. That directory ignores itself, so nothing appears in your
repository status and there is nothing to add to a `.gitignore`. An agent
running on stale instructions still leaves a complete trail.

```bash
umoja activity              # recent commands
umoja activity --changes    # recent file mutations, unverified ones flagged
umoja activity --changes -n 50
```

`umoja activity --changes` marks anything written with no checker as
`UNVERIFIED` and totals them. A growing count there is the signal that edits
are going in unchecked.

**The tool will ask you for a report.** After every 5 changes with no report
filed, any `umoja` command prints a reminder to stderr naming the exact call
to make. It asks once per batch, not on every command, and filing a report
resets the count. *Nothing to report* is a fine answer — but say so by filing
nothing, not by ignoring a real defect you worked around.


## 📖 Rhai in one screen

1. **Variables**: `let count = 0; const MAX = 100;` — `new` is a reserved word.
2. **Strings**: `"hello"`, template `` `result: ${x}` ``, raw `r#"..."#`.
   **Use `r###"..."###` when the payload itself contains `r#"`** — nested `"#`
   ends the literal early and you get "open string is not terminated".
3. **Maps**: `#{ name: "Alice", score: 95 }`, access `user.name` / `user["name"]`.
4. **Arrays**: `[1, 2, 3]`, `.push()`, `.len()`, `.pop()`.
5. **Closures**: `|x| x.score > 80`.
6. **Control flow**: `if x > 0 { } else { }`, `for item in list { }`, `for i in range(0, 10) { }`.
7. A missing map key reads as `()`, which is **not iterable** — `for e in res.errors`
   throws if `errors` is absent. Guarded results always include it; your own
   maps may not.

## 🗂️ Reading without drowning

```bash
umoja kernel exec 'print(outline("src/lib.rs"));'          # shape, not text
umoja kernel exec 'print(slice_lines("src/lib.rs", 40, 80));'
umoja kernel exec 'let f = load("crates/**/*.rs"); print(f.len());'
umoja kernel exec 'let h = grep("TODO", "**/*.md"); print(h.len());'
```
Load once, reduce in the kernel, print the answer — never the data. Dataset
builtins do the reducing natively: `sum_by`, `avg_by`, `min_by`, `max_by`,
`group_by`, `count_by`, `sort_by`, `sort_by_desc`, `filter_eq`, `filter_neq`,
`filter_contains`, `pluck`, `unique`, `unique_by`, `take_n`, `drop_n`,
`find_first`, `read_lines`, `parse_csv`, `parse_tsv`, `difference`, `intersect`,
`counter`, `parse_int`, `parse_float`, `json_parse`, `to_json`, `sh_status`.

## ✅ Goals — the checklist that costs no tokens

Steps live in SQLite and render as a one-line progress indicator instead of
being restated in the transcript every turn.

```bash
umoja goal set "Refactor the file-system module"
umoja goal add "Write the failing test"
umoja goal add "Apply the minimal fix"
umoja goal check 1
umoja goal status        # 1/2 steps complete (50%)
umoja goal checklist     # plain markdown
```

## 📬 Talk to the other agents

Umoja has a real message bus; use it instead of re-deriving what a sibling
already knows.

```bash
umoja send --to parent "walk.rs split landed; expr.rs owns the type rules now"
umoja send --to peer --name api-reviewer "please re-check crates/domain/src/ast.rs"
umoja inbox                     # what is waiting for you
umoja inbox --consume           # read and clear
```

Roles: `parent`, `child`, `sibling`, `peer` (named), `all` (your family only).
Check `umoja inbox` at the start of a turn when you are part of a fleet.

## 🧠 Memory, subagents, continuity

```bash
umoja harness remember --evidence "test trace" tdd "the false-proof test must be written first"
umoja harness search "false proof"
umoja harness rollback <id>

umoja agent call reviewer "check crates/domain for invariant leaks"

umoja heartbeat add --every 10m "re-run the corpus score"
umoja schedule add --cron "0 9 * * *" "summarise yesterday"
umoja tick

umoja compact                   # fold the transcript when context tightens
```

---

## 🔴🟢 Playbook: Hypothesize, Test & Drill-Down Testing Workflow

A disciplined, token-efficient testing lifecycle for agents combining structural analysis, targeted assertions, and guarded execution:

```
 [ 1. HYPOTHESIZE & SEARCH ] ──► [ 2. STAGE RED TEST ] ──► [ 3. DRILL-DOWN IMPLEMENTATION ]
 (ast_grep, outline, load)        (try_add_to_mod, sh_status)   (try_replace_fn, ast_rewrite)
            ▲                                                                   │
            │                                                                   ▼
 [ 5. PERSIST & REMEMBER ]   ◄─── [ 4. VERIFY SUITE ]  ◄────────────────────────┘
(log_action & remember)          (sh_status workspace check)
```

### Step 1: Formulate Hypothesis & Drill Down Structurally
Locate candidate symbols and AST nodes without dumping entire files into context:
```bash
umoja kernel exec '
// Find candidate function signatures structurally
let hits = ast_grep("fn $NAME($$$ARGS) -> $RET { $$$BODY }", "crates/**/*.rs", "rust");
for m in hits.matches {
    if m.text.contains("calc_multiplier") {
        print(`found in ${m.file}:${m.line}`);
    }
}
'
```

### Step 2: Stage the Red Test with Structural Placement (`try_add_to_mod`)
Insert the failing test cleanly inside `mod tests { ... }`. Test code is checked by `cargo check --tests` automatically during insertion:
```bash
umoja kernel exec '
let test_code = r###"
    #[test]
    fn test_fractional_budget_scaling() {
        assert_eq!(calc_multiplier(100, 2), 200);
    }
"###;

let r = try_add_to_mod("crates/umoja-domain/src/token.rs", "tests", test_code);
print(`test placed: ${r.ok}`);

// Verify Red State using the oracle without flooding the context:
let check = sh_status("cargo test test_fractional_budget_scaling");
if !check.ok {
    print(`Confirmed RED (exit ${check.code}): expected failure`);
}
'
```

### Step 3: Targeted Guarded Implementation (`try_replace_fn` / `ast_rewrite`)
Implement the fix at the precise symbol level. Guarded editors immediately roll back if diagnostics fail:
```bash
umoja kernel exec '
let new_fn = r###"
pub fn calc_multiplier(base: u32, factor: u32) -> u32 {
    base * factor
}
"###;

let r = try_replace_fn("crates/umoja-domain/src/token.rs", "calc_multiplier", new_fn);
print(`applied: ${r.ok} (${r.checker})`);
'
```

### Step 4: Verify Green & Full Workspace Suite (`sh_status`)
Confirm the target test passes and verify zero regressions across the workspace:
```bash
umoja kernel exec '
let target = sh_status("cargo test test_fractional_budget_scaling");
assert(target.ok, "Target test must pass");

let suite = sh_status("cargo test --workspace");
assert(suite.ok, "Full test suite must pass");
print("All tests GREEN across workspace!");
'
```

### Step 5: Log Rationale & Remember Harness Evidence
Log the exact change reasoning to the journal and commit evidence into harness memory:
```bash
umoja kernel exec '
log_action("implemented calc_multiplier with overflow protection", "crates/umoja-domain",
           "needed for token budget compaction calculations");
'
umoja harness remember --evidence "92/92 tests passed in cargo test --workspace" "token_budget" "calc_multiplier handles integer scaling"
```

## Kernel lifecycle

```bash
umoja kernel exec 'code'      # run; the last expression prints
umoja kernel vars             # names, types, sizes — never values
umoja kernel status           # cold | ready | dead
umoja kernel reset            # empty the namespace, keep the process
umoja kernel stop             # end it
```

An exception is an outcome, not a catastrophe: you get the error, and **the
namespace is untouched** — whatever you loaded ten minutes ago is still there.

