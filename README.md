# UMOJA

**U**moja **M**anages **O**rchestrated **J**oint **A**gents

*Umoja* — Swahili for "unity," the first principle of Kwanzaa: working as one
toward a shared purpose. This brings unified agent capabilities as one fast,
pure Rust binary (`umoja`), usable from Claude Code, opencode, Gemini/AGY, or
a plain shell.

```bash
./install.sh
umoja kernel exec 'let rows = load("huge.jsonl"); rows.len()'
umoja kernel exec 'rows.filter(|r| r.status == "error").len()' # persistent state across turns
```

- **A persistent kernel** — Rhai (in-process), Python, Node, or Shell. Variables survive between
  tool calls, so large data is loaded once and queried incrementally instead of
  being dumped into the context window.
- **A guarded file toolkit in the namespace** — `load`, `grep`, `outline`, `head`,
  `slice_lines`, `try_edit`, `try_replace_lines`, `try_replace_fn`, `ast_rewrite`, `create_module`, `sh`.
- **A continual harness** — evidence-backed memories, notes, skill and subagent
  specs, with a before/after paper trail and one-command rollback.
  `umoja harness remember` records findings; `umoja activity` logs every mutation.
- **Subagents & Messaging** — `umoja agent spawn` / subagent dispatch with structured IPC.
- **Goals, heartbeats, schedules, autonomous gates, compaction** — work that
  outlives the turn, with budgets that are never reported as completion.

---

## 🧪 Agentic Test-Driven Development (TDD)

UMOJA provides a guarded Red-Green-Refactor development cycle designed specifically for autonomous agents to prevent workspace corruption and guarantee verifiable code changes.

```
       [ 1. RED ]                       [ 2. VERIFY ]                     [ 3. GREEN ]
  Write Failing Test        ───►     Confirm Failure Cause    ───►    Guarded Implementation
 (Unit / Scratch Test)                 (in-kernel check)                (try_edit / rollback)
          ▲                                                                       │
          │                                                                       ▼
 [ 5. LOG & PERSIST ]       ◄───    [ 4. REGRESSION SUITE ]   ◄───────────────────┘
(log_action & remember)               (cargo test --workspace)
```

### 1. The Red-Green Cycle

1. **🔴 Red — Write the Failing Test First:**
   Add test coverage using guarded editors before touching production code:
   ```bash
   umoja kernel exec '
   let test_fn = r#"
   #[test]
   fn test_budget_multiplier() {
       assert_eq!(calc_multiplier(10, 2), 20);
   }
   "#;
   let res = try_replace_lines("crates/umoja-domain/src/token.rs", 50, 50, test_fn);
   print(`test inserted: ${res.ok}`);
   '
   ```

2. **🔍 Verify Test Failure:**
   Run the test inside the kernel and verify it fails for the expected reason (e.g. missing function or incorrect logic):
   ```bash
   umoja kernel exec 'sh("cargo test test_budget_multiplier")'
   ```

3. **🟢 Green — Implement with Guarded Editors:**
   Implement the fix using `try_edit`, `try_replace_fn`, or `ast_rewrite`. If the change fails compilation or diagnostics, it is rolled back instantly:
   ```bash
   umoja kernel exec '
   let res = try_replace_fn("crates/umoja-domain/src/token.rs", "calc_multiplier",
   "pub fn calc_multiplier(base: u32, factor: u32) -> u32 { base * factor }");
   print(`applied: ${res.ok} (${res.checker})`);
   '
   ```

4. **🛡️ Full Suite Regression Check:**
   Ensure zero regressions across the workspace:
   ```bash
   umoja kernel exec 'sh("cargo test --workspace")'
   ```

5. **📝 Log Intent & Evidence:**
   Record what changed and why to the activity journal and harness:
   ```bash
   umoja kernel exec '
   log_action("implemented calc_multiplier", "crates/umoja-domain",
              "needed for autonomous turn compaction calculations");
   '
   umoja harness remember --topic "tokens" "calc_multiplier handles integer scaling"
   ```

---

### 🔬 Scratch Tests for Codebase Exploration & Reproduction

When diagnosing a bug, testing complex interactions, or validating a multi-crate workflow, agents can create **scratch tests** that compile and link against the full codebase without modifying core source files:

#### A. Integration Scratch Tests (`tests/scratch_*.rs`)
In Cargo workspaces, files added to `tests/` automatically compile as integration test crates with access to all public crate APIs:

```bash
umoja kernel exec '
let scratch_code = r#"
use umoja_domain::ids::SessionId;
use umoja_infra::activity::ActivityStore;

#[test]
fn test_scratch_session_flow() {
    let session = SessionId::new();
    assert!(!session.to_string().is_empty());
}
"#;
// Create isolated scratch test
let res = try_replace_lines("tests/scratch_session_test.rs", 1, 1, scratch_code);
'
```

Run the scratch test in isolation:
```bash
umoja kernel exec 'sh("cargo test --test scratch_session_test")'
```

#### B. In-Kernel Dynamic Test Harness
For fast reproduction and data verification, agents can run dynamic tests in Rhai / Shell without generating test binaries:
```bash
umoja kernel exec '
let res = sh("cargo run --bin umoja -- activity --changes -n 5");
assert(res.contains("UNVERIFIED") || res.contains("activity"), "Activity log format matched");
print("In-kernel assertion passed!");
'
```

#### C. Scratch Test Lifecycle & Cleanup
- **Reproduce First:** Write the scratch test reproducing the issue or edge case.
- **Iterate:** Use guarded edits on the codebase until the scratch test passes.
- **Promote or Remove:** Once verified, either promote the test into the permanent test suite (e.g. `tests/session_tests.rs`) or cleanly remove the scratch file using kernel commands.

---
Start at [SKILL.md](SKILL.md). Design notes in
[references/architecture.md](references/architecture.md).

MIT.

