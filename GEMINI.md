# Antigravity (AGY) & UMOJA Rules

## What is Rhai? (Quick Syntax Guide for AI Agents)

**Rhai** is an embedded scripting language for Rust with syntax combining JavaScript and Rust:
- **Variables**: `let x = 10;`, `const PI = 3.14;`
- **Strings**: Double quotes `"text"` or template literals `` `val: ${x}` `` (Single quotes `'c'` are characters only).
- **Maps/Objects**: `#{ name: "Alice", score: 95 }`
- **Arrays**: `let list = [1, 2, 3]; list.push(4); list.len()`
- **Lambdas**: `|x| x.score > 80`
- **Loops/If**: `if cond { ... }`, `for item in list { ... }`, `for i in range(0, 10) { ... }`

## 🎯 The Agent Navigation Playbook (Best Practice Workflows)

1. **Targeted Code Navigation (Climb the Ladder)**:
   - **Step 1 (Find)**: `let hits = grep("fn target_func", "crates/**/*.rs");`
   - **Step 2 (Shape)**: `print(outline("path/to/file.rs"));`
   - **Step 3 (Line Range)**: `print(slice_lines("path/to/file.rs", 120, 155));` (read only the target range!)
   - **Step 4 (Edit & Verify)**: `edit("path/to/file.rs", "old", "new"); sh("cargo test");`

2. **Multi-File Batch Refactoring**:
   - `let files = load("crates/**/*.rs");`
   - `let matches = files.filter_by_content("target_pattern");`
   - `for f in matches { edit(f.path, "old", "new"); }`

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
