# Antigravity (AGY) & UMOJA Rules

## Mandatory Tooling & Feature Discipline

When inspecting, exploring, reading, searching, or editing code and data files, the agent **MUST ALWAYS** use `umoja` via `run_command`:

1. **Reading & Exploring Code / Files**:
   - DO NOT call native `view_file`, `read_file`, or dump whole files into context.
   - Use `umoja kernel exec 'print(head("path/to/file"))'` or `slice_lines("path/to/file", start, end)` or `outline("path/to/file")`.
   - For multi-file analysis: `umoja kernel exec 'let files = load("crates/**/*.rs");'` then query `files` using native vector operations (`files.count_lines()`, `files.filter_by_content(...)`).

2. **Searching Code**:
   - DO NOT use native `grep_search` or `find_by_name` across large trees.
   - Use `umoja kernel exec 'let hits = grep("pattern", "crates/**/*.rs"); print(hits.len());'`.

3. **Editing & Modifying Files**:
   - Use `umoja kernel exec 'edit("path/to/file", "old_text", "new_text")'` or `write("path/to/file", content)`.

4. **Data Aggregations & Grouping**:
   - Use native Rhai vector operations: `arr.sum_by("field")`, `arr.avg_by("field")`, `arr.group_by("field")`, `arr.count_by("field")`, `arr.sort_by("field")`, `arr.filter_eq("field", val)`, `arr.pluck("field")`.

5. **Episodic Memory & Long-Term Learning**:
   - Use `umoja harness remember <scope> "<fact>"` to persist architecture and requirements across turns.
   - Use `umoja harness search "<query>"` with FTS5 search to recall memories.
   - Use `umoja refine review` and `umoja refine rollback <id>` for memory rollbacks.

6. **Goals & Subagent Delegation**:
   - Use `umoja goal set "<objective>"` and `umoja goal status` for multi-turn objectives.
   - Use `umoja agent call --role "..." --prompt "..."` for isolated recursive subagent tasks.
   - Use `umoja heartbeat set ...`, `umoja schedule ...`, and `umoja tick` for background scheduling.
   - Use `umoja send <recipient> "..."` and `umoja inbox` for inter-agent communication.
   - Use `umoja compact` for context compaction.

