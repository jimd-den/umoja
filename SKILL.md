---
name: umoja
description: UMOJA (Umoja Manages Orchestrated Joint Agents) — MANDATORY FILE & KERNEL ENGINE FOR ANTIGRAVITY (AGY) & CLAUDE CODE. Always activate this skill for ANY task involving reading, inspecting, searching, editing, or creating files, running tests, exploring codebases, or handling multi-turn context. The agent MUST NOT use native view_file, grep_search, or edit tools to browse or edit code; instead, use `umoja kernel exec` (with Rhai functions `head`, `outline`, `slice_lines`, `load`, `grep`, `edit`, `write`, `sum_by`, `group_by`, `count_by`, `sort_by`, `filter_eq`, `pluck`), `umoja agent call` (subagents/RLM), `umoja harness remember/search/rollback`, `umoja goal set/status`, `umoja heartbeat/schedule/tick`, `umoja send/inbox`, `umoja compact`, and SQLite memory with FTS5 search.
license: MIT
compatibility: Linux or macOS. Rust toolchain to build. Pure Rust (Rhai) in-process kernel, embedded SQLite database. No Python or external socket daemon needed. No network access required.
---

# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents. *Umoja* is Swahili for "unity" — one persistent namespace, many agents, pulling in one direction.

---

## 📖 What is Rhai? (Quick Syntax Guide for AI Agents)

**Rhai** is an embedded scripting language for Rust with a clean syntax combining **JavaScript** and **Rust**.

### Rhai Syntax Rules:
1. **Variables**:
   - Always declare with `let` or `const`: `let count = 0; const MAX = 100;`
2. **Strings**:
   - Use double quotes `"hello"` or backticks `` `result: ${x}` ``.
   - **Important**: Single quotes `'c'` are for single `char` characters only. Use `"string"` for text.
3. **Object Maps**:
   - Maps use the leading hash syntax: `#{ name: "Alice", score: 95, active: true }`
   - Access fields with dot notation or index: `user.name` or `user["name"]`
4. **Arrays**:
   - `let list = [1, 2, 3];`
   - Methods: `list.push(4)`, `list.len()`, `list[0]`, `list.pop()`
5. **Closures / Lambdas**:
   - Rust-style pipes: `|x| x.score > 80`
   - Example: `let passing = users.filter(|u| u.score >= 70);`
6. **Control Flow**:
   - `if x > 0 { ... } else { ... }`
   - `for item in list { ... }`
   - `for i in range(0, 10) { ... }`
   - `while count < 5 { ... }`

---

## 🛑 MANDATORY RULE: The Kernel Is How You Touch Files

This rule is **strictly mandatory and enforced** across all agent sessions:

| Instead of (DO NOT USE) | Use (MANDATORY) |
|---|---|
| Native `view_file` / `Read` | `umoja kernel exec 'print(head("path/to/file"))'` or `outline` or `slice_lines` |
| Native `read_file` / multi-read | `umoja kernel exec 'let files = load("src/**/*.rs");'` then query |
| Native `grep_search` / `find_by_name` | `umoja kernel exec 'let hits = grep("pattern", "crates/**/*.rs"); print(hits.len());'` |
| Native `write_to_file` / `Write` | `umoja kernel exec 'write("path/to/file", text)'` |
| Native `replace_file_content` / `Edit` | `umoja kernel exec 'edit("path/to/file", "old_text", "new_text")'` |
| Native command execution for data | `umoja kernel exec 'let out = sh("git status"); print(out);'` |

**Why this rule is exclusive:**
Reading files directly with native view/read tools dumps raw content into conversation context permanently, causing context bloat and token exhaustion. `umoja` loads files into an in-process pure Rust Rhai kernel, keeping raw tokens out of context, and returns only the precise answers, slices, or summaries needed.

---

## ⚡ Core Philosophy: Load Into Variables, Print Only Answers

```bash
# 1. Load entire dataset or codebase into persistent kernel memory (prints nothing)
umoja kernel exec 'let files = load("crates/**/*.rs");'

# 2. Inspect shape and metrics in native Rust
umoja kernel exec 'print("Files: " + files.len() + ", Total lines: " + files.count_lines());'

# 3. Query or filter in memory across subsequent tool turns (variable `files` remains bound)
umoja kernel exec 'let tests = files.filter_by_content("test"); print("Files with tests: " + tests.len());'
```

---

## 🛠️ Complete Rhai Built-In Function Reference

### 1. Codebase Exploration & File I/O
* **`load(glob_pattern)`**: Recursively globs files into an array of objects `[ #{ path, content, lines, size }, ... ]`. Also parses `.json` files directly.
* **`head(path, [lines=50])`**: Returns first $N$ lines of a file as a string.
* **`tail(path, [lines=50])`**: Returns last $N$ lines of a file as a string.
* **`slice_lines(path, start, end)`**: Returns lines `start` to `end` (1-indexed, inclusive).
* **`outline(path)`**: Extracts structs, functions, enums, traits, classes, methods, and markdown headings with line numbers.
* **`grep(pattern, [target_path])`**: Fast pattern matching returning `[ #{ path, line, content }, ... ]`.
* **`read(path)`**: Reads entire file as a string.
* **`write(path, text)`**: Writes string to file, creating parent directories automatically.
* **`edit(path, old_text, new_text)`**: Performs exact single-occurrence replacement.
* **`sh(command)`**: Runs a shell command and captures stdout into a string.

### 2. High-Performance Native Dataset & Vector Operations
* **`arr.sum_by("field")`**: Fast compiled numeric sum across maps.
* **`arr.avg_by("field")`**: Computes average value.
* **`arr.min_by("field")` & `arr.max_by("field")`**: Finds minimum/maximum objects.
* **`arr.count_by("field")`**: Returns frequency count histogram `#{ "category": count }`.
* **`arr.group_by("field")`**: Groups array into map of arrays `#{ "key": [ ... ] }`.
* **`arr.sort_by("field")` / `arr.sort_by_desc("field")`**: Native QuickSort.
* **`arr.filter_eq("field", value)`**: Fast equality filtering.
* **`arr.filter_contains("field", "substring")`**: Fast substring search.
* **`arr.pluck("field")`**: Extracts single field into an array of values.
* **`arr.unique()` / `arr.unique_by("field")`**: Deduplicates items.
* **`arr.take_n(n)` / `arr.drop_n(n)`**: Slices array.

### 3. JSON Parsing & Serialization
* **`parse_json(string)` / `json_parse(string)`**: Converts JSON string to Rhai Map/Array.
* **`to_json(value)` / `json_stringify(value)`**: Formats Rhai data structures to JSON.

---

## 🧠 Complete UMOJA Subsystem Guide

### 1. Continual Learning & Memory (`harness` & `refine`)
Persist important facts, project conventions, and architecture decisions across turns and sessions:
```bash
# Remember a durable architectural fact
umoja harness remember architecture "Uses Clean Architecture with domain, app, infra, and cli crates"

# Search memory using SQLite FTS5 full-text search
umoja harness search "Clean Architecture"

# List all stored memories
umoja harness list

# Review or rollback a bad memory revision
umoja refine review
umoja refine rollback <id>
```

### 2. Persistent Objectives & Goals (`goal`)
Track multi-turn objectives so context is never lost:
```bash
# Set active goal
umoja goal set "Refactor kernel builtins into clean submodules and verify benchmarks"

# Check goal progress
umoja goal status
umoja goal list
```

### 3. Recursive Delegation & Subagents (`agent`)
Spawn isolated child agents for deep sub-tasks:
```bash
# Run a subagent for research or deep exploration
umoja agent call --role "Codebase Researcher" --prompt "Explore all SQL stores in crates/umoja-infra"
```

### 4. Background Heartbeats, Timers & Scheduling (`heartbeat`, `schedule`, `tick`)
```bash
# Set recurring periodic instruction
umoja heartbeat set 30m "Check git status and compile tests"

# Set one-time delayed reminder
umoja schedule 10m "Verify benchmark results"

# Deliver due scheduled tasks
umoja tick
```

### 5. Multi-Agent Messaging Bus (`send`, `inbox`, `roster`)
```bash
# List all active sessions
umoja roster

# Send a structured message to another session
umoja send worker-1 "Dataset loaded in kernel variable 'users', proceed with aggregation"

# Check inbox for incoming messages
umoja inbox
```

### 6. Context Compaction (`compact`)
```bash
# Condense long conversation logs and extract instruction outlines
umoja compact
```

---

## 🧗 Climb the Ladder Before You Print

1. **`grep(...)`** — You know what you are looking for.
2. **`outline(path)`** — You need the structural shape without full bodies.
3. **`slice_lines(path, start, end)`** — `outline` told you exactly which line range to inspect.
4. **`head(path, 20)`** — A quick peek near the top is sufficient.
5. **Full print** — The last resort, only for small files when strictly necessary.
