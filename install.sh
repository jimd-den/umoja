#!/usr/bin/env bash
# Builds `pa` and makes it reachable from every harness on this machine.
#
# Idempotent: safe to re-run after editing the source or pulling changes.

set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${PA_BIN_DIR:-$HOME/.local/bin}"

say() { printf '  %s\n' "$*"; }

echo "prime-agent"
echo

if ! command -v cargo >/dev/null 2>&1; then
  echo "  cargo is not on PATH. Install Rust from https://rustup.rs and re-run." >&2
  exit 1
fi

say "building (release)…"
cargo build --release --manifest-path "$SKILL_DIR/Cargo.toml" --quiet

mkdir -p "$BIN_DIR"
ln -sf "$SKILL_DIR/target/release/pa" "$BIN_DIR/pa"
say "linked $BIN_DIR/pa"

# The skill itself is discoverable by anything that reads the cross-harness
# location; a symlink makes it visible to Claude Code without a second copy.
CLAUDE_SKILLS="$HOME/.claude/skills"
if [ -d "$CLAUDE_SKILLS" ] && [ ! -e "$CLAUDE_SKILLS/prime-agent" ]; then
  ln -s "$SKILL_DIR" "$CLAUDE_SKILLS/prime-agent"
  say "linked $CLAUDE_SKILLS/prime-agent"
fi

echo
if ! command -v pa >/dev/null 2>&1; then
  say "note: $BIN_DIR is not on your PATH. Add it:"
  say "      export PATH=\"$BIN_DIR:\$PATH\""
  echo
fi

# Report what is actually available rather than assuming.
for tool in claude opencode python3 node; do
  if command -v "$tool" >/dev/null 2>&1; then
    say "found    $tool"
  else
    say "missing  $tool (optional)"
  fi
done

echo
say "done. Try:  pa status"
say "            pa kernel exec 'x = 41; x + 1'"
