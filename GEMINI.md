# Antigravity (AGY) & UMOJA Rules

Every file operation goes through `umoja kernel exec`. Never use native
`view_file` / `Read`, `grep_search`, `write_to_file` / `Write`, or
`replace_file_content` / `Edit`.

## 1. Fast Structural Inspection & Symbol Readers
- Never grep blindly or dump raw source code into context.
- Use `ast_find_fn`, `ast_find_calls`, `ast_find_struct`, `ast_grep`, `read_fn`, `read_impl`, `enclosing`.

## 2. In-Memory Pre-Flight Validation & Guarded Editors
- Pre-validate with `validate_syntax(code, "rust")` before proposing disk writes.
- Use only guarded editors: `try_replace_fn`, `try_edit`, `try_replace_lines`, `try_add_to_mod`, `ast_rewrite`, `create_module`.
- Rejections are guaranteed to carry `errors` array.

## 3. Red-Green TDD & Hypothesis Test Oracle
- Stage failing tests first with `scaffold_test` or `create_scratch_test`.
- Verify red state with `run_test_oracle(test_name)` to extract exact assertion panics (`left == right`) and locations.
- Make it pass with guarded edits, run `cargo test --workspace`, then `log_action`.

## 4. NVIDIA AVO Evolutionary Optimization & Lineage
- Track candidate progressions via `lineage_history`, `lineage_best`, `lineage_commit`.
- Benchmark with `profile_benchmark(cmd)`.
- Inspect CLI state with `umoja evolve lineage/best/status <target>`.

## 5. Kernel Data Reduction & Mandatory Action Logging
- Reduce in the kernel (`slice_lines`, `counter`, `difference`, `read_lines`).
- Always log actions with `log_action("action", "target", "falsifiable reason")`.

