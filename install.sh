#!/usr/bin/env bash
# Builds `umoja` and makes it reachable from every harness on this machine.
#
# Idempotent: safe to re-run after editing the source or pulling changes.

set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${PA_BIN_DIR:-$HOME/.local/bin}"

say() { printf '  %s\n' "$*"; }

echo "umoja"
echo

if ! command -v cargo >/dev/null 2>&1; then
  echo "  cargo is not on PATH. Install Rust from https://rustup.rs and re-run." >&2
  exit 1
fi

say "building (release)…"
cargo build --release --manifest-path "$SKILL_DIR/Cargo.toml" --quiet

mkdir -p "$BIN_DIR"
ln -sf "$SKILL_DIR/target/release/umoja" "$BIN_DIR/umoja"
ln -sf "$SKILL_DIR/target/release/umoja" "$BIN_DIR/pa"
say "linked $BIN_DIR/umoja (and $BIN_DIR/pa)"

# The skill itself is discoverable by anything that reads the cross-harness
# location; a symlink makes it visible to Claude Code and Antigravity (AGY) without a second copy.
CLAUDE_SKILLS="$HOME/.claude/skills"
if [ -d "$CLAUDE_SKILLS" ] && [ ! -e "$CLAUDE_SKILLS/umoja" ]; then
  ln -s "$SKILL_DIR" "$CLAUDE_SKILLS/umoja"
  say "linked $CLAUDE_SKILLS/umoja"
fi

# Antigravity (AGY) skills discovery
AGY_GLOBAL_SKILLS="$HOME/.gemini/config/skills"
AGY_CLI_SKILLS="$HOME/.gemini/antigravity-cli/skills"
mkdir -p "$AGY_GLOBAL_SKILLS" "$AGY_CLI_SKILLS"
if [ ! -e "$AGY_GLOBAL_SKILLS/umoja" ]; then
  ln -s "$SKILL_DIR" "$AGY_GLOBAL_SKILLS/umoja"
  say "linked $AGY_GLOBAL_SKILLS/umoja (AGY)"
fi
if [ ! -e "$AGY_CLI_SKILLS/umoja" ]; then
  ln -s "$SKILL_DIR" "$AGY_CLI_SKILLS/umoja"
  say "linked $AGY_CLI_SKILLS/umoja (AGY)"
fi

# opencode command
OPENCODE_CMD="${XDG_CONFIG_HOME:-$HOME/.config}/opencode/command"
if command -v opencode >/dev/null 2>&1 && [ ! -e "$OPENCODE_CMD/umoja.md" ]; then
  mkdir -p "$OPENCODE_CMD"
  cat > "$OPENCODE_CMD/umoja.md" <<EOC
---
description: UMOJA — Pure Rust persistent kernel, continual harness, subagents, goals, schedules, autonomous gates
---

Read $SKILL_DIR/SKILL.md and follow it, loading the files under
$SKILL_DIR/references/ only as the task requires them.

The \`umoja\` binary is already installed. Its most important habit: load large
data into the pure Rust kernel with \`umoja kernel exec\` and print only the reduced answer,
so the data never enters this conversation.

Task: \$ARGUMENTS
EOC
  say "wrote $OPENCODE_CMD/umoja.md  (use: /umoja)"
fi

echo
if ! command -v umoja >/dev/null 2>&1; then
  say "note: $BIN_DIR is not on your PATH. Add it:"
  say "      export PATH=\"$BIN_DIR:\$PATH\""
  echo
fi

# Report what is actually available rather than assuming.
for tool in claude opencode node; do
  if command -v "$tool" >/dev/null 2>&1; then
    say "found    $tool"
  else
    say "missing  $tool (optional)"
  fi
done

echo
say "done. Try:  umoja status"
say "            umoja kernel exec 'let x = 41; x + 1'"
