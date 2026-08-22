# Antigravity (AGY) & UMOJA Rules

## Mandatory Tooling Discipline

When inspecting, exploring, reading, searching, or editing code and data files, the agent **MUST ALWAYS** use `umoja` via `run_command`:

1. **Reading & Exploring Code / Files**:
   - DO NOT call native `view_file`, `read_file`, or dump whole files into context.
   - Use `umoja kernel exec 'print(head("path/to/file"))'` or `slice_lines("path/to/file", start, end)` or `outline("path/to/file")`.
   - For multi-file analysis: `umoja kernel exec 'let files = load("crates/**/*.rs");'` then query `files`.

2. **Searching Code**:
   - DO NOT use native `grep_search` or `find_by_name` across large trees.
   - Use `umoja kernel exec 'grep("pattern")'` over loaded datasets.

3. **Editing & Modifying Files**:
   - Use `umoja kernel exec 'edit("path/to/file", old_text, new_text)'` or `write("path/to/file", content)`.

4. **Context & Execution Efficiency**:
   - Always load data into persistent variables in the pure Rust Rhai kernel (`umoja kernel exec '...'`).
   - Print only the reduced or summarized answer to keep raw tokens out of the conversation context.
   - Use `umoja status`, `umoja harness remember/list`, `umoja goal set/status`, `umoja tick`, and `umoja agent call` for subagents.
