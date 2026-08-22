---
name: umoja
description: UMOJA (Umoja Manages Orchestrated Joint Agents) — MANDATORY FILE & KERNEL ENGINE FOR ANTIGRAVITY (AGY) & CLAUDE CODE. Always activate this skill for ANY task involving reading, inspecting, searching, editing, or creating files, running tests, exploring codebases, or handling multi-turn context. The agent MUST NOT use native view_file, grep_search, or edit tools to browse or edit code; instead, use `umoja kernel exec` (with Rhai functions `head`, `outline`, `slice_lines`, `load`, `grep`, `edit`, `replace_lines`, `replace_fn`, `write_b64`, `sum_by`, `group_by`, `count_by`, `sort_by`, `filter_eq`, `pluck`), `umoja agent call` (subagents/RLM), `umoja harness remember/search/rollback`, `umoja goal set/status`, `umoja heartbeat/schedule/tick`, `umoja send/inbox`, `umoja compact`, and SQLite memory with FTS5 search.
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
2. **Strings & Raw Literals**:
   - Use double quotes `"hello"` or template literals `` `result: ${x}` ``.
   - **Rust-Style Raw Strings**: Use `r#"..."#` or `r###"..."###` to paste code blocks containing unescaped quotes, regexes, and backslashes with zero escaping!
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

---

## 🛠️ Advanced Clean Editing Capabilities (Escape-Proof & Robust)

To avoid escaping collisions, quoting bugs, or whitespace mismatches when editing files:

### 1. Dedicated Line-Range Replacement (`replace_lines` / `insert_at_line`)
Replace lines $N$ through $M$ directly (1-indexed, inclusive) without needing to match exact substrings:
```bash
# Replace lines 120 to 160 directly
umoja kernel exec 'replace_lines("src/lib.rs", 120, 160, new_body);'

# Insert lines before line 45
umoja kernel exec 'insert_at_line("src/lib.rs", 45, "use std::collections::HashMap;");'

# Insert relative to an anchor line
umoja kernel exec 'insert_after("src/lib.rs", "fn init()", "    logger::setup();");'
```

### 2. Rust-Style Raw Strings (`r#"..."#`)
Paste complex code with quotes, single quotes, regexes, and backslashes without escaping:
```bash
umoja kernel exec '
let patch = r#"
    match esc {
        '\\' | '"' | '\'' => out.push(esc),
        other => return Err(format!("unknown escape `\\{other}`")),
    }
"#;
replace_lines("crates/umoja-infra/src/parser.rs", 55, 60, patch);
'
```

### 3. Symbol / Function-Level Structural Edits (`replace_fn` / `replace_struct`)
Target and replace entire functions or structs by symbol name:
```bash
# Replaces fn old_method or pub fn old_method and its balanced braces
umoja kernel exec 'replace_fn("src/auth.rs", "verify_token", new_fn_code);'

# Replaces struct UserSession definition
umoja kernel exec 'replace_struct("src/models.rs", "UserSession", new_struct_code);'
```

### 4. Base64 Safe Stream Ingestion (`write_b64` / `replace_lines_b64`)
When payloads contain fragile shell characters, pass base64 streams for 100% byte fidelity:
```bash
umoja kernel exec 'write_b64("path/to/file", "SGVsbG8gV29ybGQ=");'
umoja kernel exec 'replace_lines_b64("path/to/file", 10, 20, "bmV3IGNvZGU=");'
```

---

## 🎯 The Agent Navigation Playbook (Best Practice Workflows)

### 🔴🟢 Playbook 0: Hypothesis-Driven TDD (Red-Green-Refactor)
```bash
# 1. State Hypothesis & Set Objective
umoja goal set "TDD: Implement unique_by dataset builtin"

# 2. RED: Write failing test first
umoja kernel exec 'replace_lines("crates/umoja-infra/src/kernel/builtins/dataset.rs", 400, 405, r#"
    #[test]
    fn test_unique_by() {
        let mut engine = Engine::new();
        register_dataset_builtins(&mut engine);
        assert!(engine.eval::<bool>("[#{id: 1}, #{id: 1}].unique_by(\"id\").len() == 1").unwrap());
    }
"#);'
umoja kernel exec 'print(sh("cargo test test_unique_by"));'

# 3. GREEN: Minimal fix
umoja kernel exec 'insert_after("crates/umoja-infra/src/kernel/builtins/dataset.rs", "register_dataset_builtins", r#"
    engine.register_fn("unique_by", |arr: &mut Array, f: &str| -> Array { unique_field(arr, f) });
"#);'
umoja kernel exec 'print(sh("cargo test test_unique_by"));'

# 4. REFACTOR & VERIFY
umoja kernel exec 'print(sh("cargo test --workspace"));'
umoja harness remember tdd "Verified unique_by invariant"
```

---

### Playbook 1: The 4-Step Targeted Code Navigation & Slicing
```bash
# Step 1: FIND symbol
umoja kernel exec 'let hits = grep("fn execute_task", "crates/**/*.rs"); for h in hits { print(`${h.path}:${h.line} -> ${h.content}`); }'

# Step 2: OUTLINE shape
umoja kernel exec 'print(outline("crates/umoja-infra/src/runner.rs"));'

# Step 3: SLICE target range
umoja kernel exec 'print(slice_lines("crates/umoja-infra/src/runner.rs", 120, 160));'

# Step 4: PRECISE LINE-RANGE EDIT
umoja kernel exec 'replace_lines("crates/umoja-infra/src/runner.rs", 130, 145, new_body);'
umoja kernel exec 'print(sh("cargo test -p umoja-infra"));'
```

---

## 🛑 MANDATORY RULE: The Kernel Is How You Touch Files

This rule is **strictly mandatory and enforced** across all agent sessions:

| Instead of (DO NOT USE) | Use (MANDATORY) |
|---|---|
| Native `view_file` / `Read` | `umoja kernel exec 'print(head("path/to/file"))'` or `outline` or `slice_lines` |
| Native `read_file` / multi-read | `umoja kernel exec 'let files = load("src/**/*.rs");'` then query |
| Native `grep_search` / `find_by_name` | `umoja kernel exec 'let hits = grep("pattern", "crates/**/*.rs"); print(hits.len());'` |
| Native `write_to_file` / `Write` | `umoja kernel exec 'write("path/to/file", text)'` or `write_b64` |
| Native `replace_file_content` / `Edit` | `umoja kernel exec 'replace_lines("path", start, end, new_text)'` or `replace_fn` or `edit` |
| Native command execution for data | `umoja kernel exec 'let out = sh("git status"); print(out);'` |
