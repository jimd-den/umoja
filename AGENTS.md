# Antigravity (AGY) & UMOJA Rules

## What is Rhai? (Quick Syntax Guide for AI Agents)

**Rhai** is an embedded scripting language for Rust with syntax combining JavaScript and Rust:
- **Variables**: `let x = 10;`, `const PI = 3.14;`
- **Strings & Raw Literals**: Double quotes `"text"` or template literals `` `val: ${x}` ``. Use `r#"raw multiline string with quotes & \backslashes"#` with zero escaping!
- **Maps/Objects**: `#{ name: "Alice", score: 95 }`
- **Arrays**: `let list = [1, 2, 3]; list.push(4); list.len()`
- **Lambdas**: `|x| x.score > 80`
- **Loops/If**: `if cond { ... }`, `for item in list { ... }`, `for i in range(0, 10) { ... }`

## 🛠️ Escape-Proof & Robust File Editing Tools

1. **Line-Range Replacement**: `replace_lines("path", start_line, end_line, new_code)` replaces 1-indexed line range directly without fragile substring matching.
2. **Line Insertion**: `insert_at_line("path", line_num, text)` and `insert_after("path", "anchor_text", text)`.
3. **Rust-Style Raw Strings**: `let patch = r#"match x { '\\' => 1 }"#;` passes unescaped code blocks cleanly.
4. **Symbol-Level AST Replacement**: `replace_fn("path", "fn_name", new_fn_body)` and `replace_struct("path", "StructName", new_struct)`.
5. **Base64 Safe Streams**: `write_b64("path", b64)` and `replace_lines_b64("path", start, end, b64)` avoid shell quoting issues.

## 🎯 The Agent Navigation Playbook (Best Practice Workflows)

0. **Hypothesis-Driven TDD (Red-Green-Refactor)**:
   - **Formulate Hypothesis**: State expected invariant before modifying code.
   - **RED**: Write failing unit test first, verify failure: `replace_lines("path/to/test.rs", ...); sh("cargo test target_test");`
   - **GREEN**: Slice implementation range, apply minimal fix, verify pass: `slice_lines(...); replace_lines(...); sh("cargo test target_test");`
   - **REFACTOR**: Clean architecture pass, run full workspace suite, persist memory: `sh("cargo test --workspace"); umoja harness remember ...`

1. **Targeted Code Navigation (Climb the Ladder)**:
   - **Step 1 (Find)**: `let hits = grep("fn target_func", "crates/**/*.rs");`
   - **Step 2 (Shape)**: `print(outline("path/to/file.rs"));`
   - **Step 3 (Line Range)**: `print(slice_lines("path/to/file.rs", 120, 155));` (read only the target range!)
   - **Step 4 (Edit & Verify)**: `replace_lines("path/to/file.rs", 120, 155, new_body); sh("cargo test");`

2. **Multi-File Batch Refactoring**:
   - `let files = load("crates/**/*.rs");`
   - `let matches = files.filter_by_content("target_pattern");`
   - `for f in matches { replace_lines(f.path, start, end, new_body); }`

3. **Data Aggregations & Grouping**:
   - `arr.sum_by("field")`, `arr.avg_by("field")`, `arr.group_by("field")`, `arr.count_by("field")`, `arr.sort_by("field")`, `arr.filter_eq("field", val)`, `arr.pluck("field")`.

4. **Episodic Memory & Long-Term Learning**:
   - `umoja harness remember <scope> "<fact>"` to persist architecture across turns.
   - `umoja harness search "<query>"` with FTS5 search to recall memories.
   - `umoja refine review` and `umoja refine rollback <id>` for memory rollbacks.

5. **Goals & Subagent Delegation**:
   - `umoja goal set "<objective>"` and `umoja goal status` for multi-turn objectives.
   - `umoja agent call --role "..." --prompt "..."` for isolated recursive subagent tasks.
   - `umoja heartbeat set ...`, `umoja schedule ...`, and `umoja tick` for background scheduling.
   - `umoja send <recipient> "..."` and `umoja inbox` for inter-agent communication.
   - `umoja compact` for context compaction.

## Mandatory Tooling & Feature Discipline

When inspecting, exploring, reading, searching, or editing code and data files, the agent **MUST ALWAYS** use `umoja` via `run_command`:
- DO NOT call native `view_file`, `read_file`, `grep_search`, `find_by_name`, `write_to_file`, or `replace_file_content`.
- Use `umoja kernel exec` with Rhai functions.
