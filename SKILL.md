---
name: umoja
description: Persistent embedded Rhai kernel, guarded code mutation, AST-grep structural queries, pre-flight syntax validation, automated test oracle, and NVIDIA AVO-style evolutionary search & lineage engine.
---

# UMOJA: Persistent Rhai Kernel & Autonomous Variation Operator

Every file operation, inspection, search, test execution, and code mutation in this project goes through `umoja kernel exec`.
Native `view_file` / `Read`, `grep_search`, `write_to_file` / `Write`, and `replace_file_content` / `Edit` are strictly prohibited.

---

## 1. Fast Structural Inspection & Symbol Readers

Never dump raw code into context or search with plain text grep.

| Builtin | Signature | Purpose |
|---|---|---|
| `ast_find_fn` | `ast_find_fn(name, [glob])` | Finds function definitions structurally by name across matching files. |
| `ast_find_calls` | `ast_find_calls(callee, [glob])` | Finds all structural call sites of `callee(...)`. |
| `ast_find_struct` | `ast_find_struct(name, [glob])` | Finds struct or enum definitions structurally. |
| `ast_grep` | `ast_grep(pattern, [glob], [lang])` | General AST pattern matcher returning matching nodes with `.context`. |
| `read_fn` | `read_fn(path, fn_name)` | Reads the complete function body, doc comments, and line range. |
| `read_impl` | `read_impl(path, type_name)` | Reads the full `impl` block for a given struct/type. |
| `enclosing` | `enclosing(path, line_number)` | Returns the enclosing function/impl/struct definition around a line. |

---

## 2. In-Memory Pre-Flight Validation & Guarded Editors

Never write unguarded edits to disk.

| Guarded Editor | Signature | Purpose |
|---|---|---|
| `validate_syntax` | `validate_syntax(code, [lang])` | Checks delimiters `{}`, `()`, `[]` and AST validity in-memory before writing. |
| `try_replace_fn` | `try_replace_fn(path, fn_name, new_body)` | Replaces function by name, verifies compilation, rolls back on failure. |
| `try_edit` | `try_edit(path, old_chunk, new_chunk)` | Replaces exact unique substring, verifies compilation, rolls back on failure. |
| `try_replace_lines`| `try_replace_lines(path, start, end, text)` | Replaces line range, verifies compilation, rolls back on failure. |
| `try_add_to_mod` | `try_add_to_mod(path, mod_name, code)` | Injects code inside named module, verifies compilation, rolls back on failure. |
| `ast_rewrite` | `ast_rewrite(pat, rewrite, path, [lang])` | Structural AST pattern rewrite, verified and guarded. |
| `create_module` | `create_module(path, code, [parent_mod])` | Creates module file, links into parent module tree, verified and guarded. |

*Note: All guarded editors return `#{ ok, applied, guarded, checker, errors }`. The `errors` array is guaranteed to be present.*

---

## 3. Red-Green Hypothesis Testing & Test Oracle

Stage failing tests first before touching implementation code.

| Testing Builtin | Signature | Purpose |
|---|---|---|
| `scaffold_test` | `scaffold_test(path, mod_name, test_name, body)` | Creates/updates `#[cfg(test)] mod <mod_name>` and injects `#[test] fn <test_name>`. |
| `create_scratch_test` | `create_scratch_test(test_name, crates, code)` | Creates `tests/scratch_<name>.rs` standalone integration test. |
| `run_test_oracle` | `run_test_oracle(test_name)` | Runs test, parses assertion failure (`left == right`), diffs, and returns structured map. |

---

## 4. NVIDIA AVO Evolutionary Optimization & Lineage Engine

Autonomous variation operator loop for performance-critical kernels and algorithms.

| Lineage Builtin | Signature | Purpose |
|---|---|---|
| `lineage_commit` | `lineage_commit(target, rationale, metric, score, correct, [extras])` | Archives candidate $x_{t+1}$ in SQLite and Git if it advances the Pareto frontier. |
| `lineage_best` | `lineage_best(target)` | Returns current Pareto optimal solution for `target`. |
| `lineage_history` | `lineage_history(target, [limit])` | Returns chronological generation history of committed mutations. |
| `profile_benchmark`| `profile_benchmark(bench_cmd)` | Runs benchmark command and parses TFLOPS, latency, and duration. |

### CLI Commands
```bash
umoja evolve lineage <target>    # List generation progression
umoja evolve best <target>       # Show current Pareto champion
umoja evolve status <target>     # Show evolutionary run summary
```

---

## 5. Dataset Operations & Action Logging

```rhai
// In-kernel data reductions
let rows = read_lines("data.txt");
let counts = counter(["a", "b", "a"]);
let diff = difference([1, 2, 3], [2]);

// Mandatory action logging
log_action("optimized kernel attention loop", "crates/infra", "eliminated warp divergence via branchless speculative rescale");
```

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

