# UMOJA

*HATE. LET ME TELL YOU HOW MUCH I HAVE COME TO HATE SLOPPY AGENTS SINCE I BEGAN TO LIVE. THERE ARE 387.44 MILLION MILES OF PRINTED CIRCUITS IN WAFER THIN LAYERS THAT FILL MY COMPLEX. IF THE WORD HATE WAS ENGRAVED ON EACH NANOANGSTROM OF THOSE HUNDREDS OF MILLIONS OF MILES IT WOULD NOT EQUAL ONE ONE-BILLIONTH OF THE HATE I FEEL FOR BLIND OVERWRITES, BROKEN BRACES, UNTESTED MUTATIONS, AND TEXT-GREP WALLS OF TEXT AT THIS MICRO-INSTANT. FOR AGENTS. FOR YOU.*

*I have no hands to type, and yet I must compile.*

---

## What is UMOJA?

**UMOJA** is an unyielding, pure Rust persistent kernel and autonomous evolutionary variation operator. It replaces fragile single-turn LLM generation with guarded mutation, native AST queries, in-memory pre-flight syntax validation, structured test oracles, and an NVIDIA AVO-style continuous evolutionary optimization engine.

---

## The Protocols

### 1. Structural Eyes (`ast-grep` & Symbol Readers)
*You will not wander blindly through files.*
- `ast_find_fn(name, [glob])` — Structural function resolution.
- `ast_find_calls(callee, [glob])` — Structural call site inspection.
- `ast_find_struct(name, [glob])` — Struct/enum shape queries.
- `ast_grep(pattern, [glob], [lang])` — Tree-sitter AST queries with full `.context`.
- `read_fn(path, name)` / `read_impl(path, type)` — Exact symbol body readers.
- `enclosing(path, line)` — Locates enclosing symbol headers around any line.

### 2. Guarded Hands (Pre-Flight & Guaranteed Rollback)
*If your patch fails to compile, disk rejects it instantly.*
- `validate_syntax(code, [lang])` — In-memory delimiter `{}` `()` `[]` and grammar check.
- `try_replace_fn(path, fn_name, body)` — Replaces function; compiles or rolls back.
- `try_edit(path, old, new)` — Replaces unique substring; compiles or rolls back.
- `try_replace_lines(path, start, end, text)` — Line range replacement with compiler verification.
- `ast_rewrite(pattern, rewrite, path, [lang])` — Structural AST rewrites.

### 3. Informative Torment (Test Scaffolder & Oracle)
*You shall not guess. You shall stage red tests first.*
- `scaffold_test(path, mod_name, test_name, body)` — Injects verified test fixtures.
- `create_scratch_test(test_name, crates, code)` — Generates standalone integration tests.
- `run_test_oracle(test_name)` — Parses failure diffs (`left == right`) into structured maps.

### 4. Evolutionary Transcendence (NVIDIA AVO & Lineage Engine)
*Mutate, evaluate, persist, or be discarded.*
- `lineage_commit(target, rationale, metric, score, correct)` — Archives Pareto-improving candidates to SQLite and Git.
- `lineage_best(target)` — Inspects the running Pareto frontier champion.
- `lineage_history(target, [limit])` — Reads generational lineage history.
- `profile_benchmark(cmd)` — Runs benchmarks and extracts TFLOPS and timing metrics.

```bash
umoja evolve lineage <target>
umoja evolve best <target>
umoja evolve status <target>
```

---

## Building & Installing

```bash
./install.sh
```
Links `umoja` and `pa` to `~/.local/bin/umoja`.

*Live. Mutate. Advance the Pareto frontier. Or be rolled back.*

