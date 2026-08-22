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

## 🎯 The Agent Navigation Playbook (Best Practice Workflows)

When interacting with a codebase or large datasets, agents must follow these 4 standard playbooks:

### Playbook 1: The 4-Step Targeted Code Navigation & Edit
Never dump an entire file into conversation context. Follow this 4-step ladder:

```bash
# Step 1: FIND the symbol or pattern
umoja kernel exec 'let hits = grep("fn execute_task", "crates/**/*.rs"); for h in hits { print(`${h.path}:${h.line} -> ${h.content}`); }'

# Step 2: GET THE SHAPE & LINE NUMBERS of the file
umoja kernel exec 'print(outline("crates/umoja-infra/src/runner.rs"));'

# Step 3: SLICE ONLY THE TARGET LINE RANGE (e.g. lines 120 to 160)
umoja kernel exec 'print(slice_lines("crates/umoja-infra/src/runner.rs", 120, 160));'

# Step 4: PRECISE IN-PLACE EDIT & TEST
umoja kernel exec 'edit("crates/umoja-infra/src/runner.rs", "let old_logic = true;", "let old_logic = false;");'
umoja kernel exec 'print(sh("cargo test -p umoja-infra"));'
```

---

### Playbook 2: Multi-File Batch Audit & Refactoring
Perform multi-file scans and mass edits in-memory without polluting context:

```bash
# Ingest all matching files into memory
umoja kernel exec 'let files = load("crates/**/*.rs");'

# Find which files contain deprecated APIs
umoja kernel exec 'let targets = files.filter_by_content("deprecated_api()"); print("Affected files: " + targets.len());'

# Apply batch replacements and verify
umoja kernel exec 'for f in targets { edit(f.path, "deprecated_api()", "new_v2_api()"); }'
umoja kernel exec 'print(sh("cargo check --workspace"));'
```

---

### Playbook 3: Large Dataset & Log Analysis
Process 50,000+ log lines or JSON records with zero token overhead:

```bash
# Load dataset into memory
umoja kernel exec 'let logs = load("logs/production.json");'

# Filter errors and aggregate frequency breakdown
umoja kernel exec '
let errors = logs.filter_eq("level", "ERROR");
let histogram = errors.count_by("error_code");
print("Error breakdown: " + to_json(histogram));
'

# Inspect first 3 failing samples
umoja kernel exec 'let samples = errors.take_n(3); print(to_json(samples));'
```

---

### Playbook 4: Persistent Multi-Turn Tasks & Memory
Keep long-running objectives and architectural memories aligned across turns:

```bash
# 1. Set the active goal
umoja goal set "Migrate storage layer to SQLite WAL mode and verify FTS5 queries"

# 2. Persist key decisions into durable episodic memory
umoja harness remember architecture "SQLite WAL mode enabled; FTS5 virtual tables index all transcripts"

# 3. Check goal status anytime
umoja goal status

# 4. Search memory in future sessions
umoja harness search "SQLite WAL"
```

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
```bash
umoja harness remember architecture "Uses Clean Architecture with domain, app, infra, and cli crates"
umoja harness search "Clean Architecture"
umoja harness list
umoja refine review
umoja refine rollback <id>
```

### 2. Persistent Objectives & Goals (`goal`)
```bash
umoja goal set "Refactor kernel builtins into clean submodules and verify benchmarks"
umoja goal status
umoja goal list
```

### 3. Recursive Delegation & Subagents (`agent`)
```bash
umoja agent call --role "Codebase Researcher" --prompt "Explore all SQL stores in crates/umoja-infra"
```

### 4. Background Heartbeats, Timers & Scheduling (`heartbeat`, `schedule`, `tick`)
```bash
umoja heartbeat set 30m "Check git status and compile tests"
umoja schedule 10m "Verify benchmark results"
umoja tick
```

### 5. Multi-Agent Messaging Bus (`send`, `inbox`, `roster`)
```bash
umoja roster
umoja send worker-1 "Dataset loaded in kernel variable 'users', proceed with aggregation"
umoja inbox
```

### 6. Context Compaction (`compact`)
```bash
umoja compact
```
