#!/usr/bin/env bash
# The local checks: secret scan, format, lint, tests and the release build.
# check-stages: quick merge release
set -euo pipefail
stage=merge
while [ $# -gt 0 ]; do
  case "$1" in
    --stage) case "${2-}" in quick | merge | release) stage=$2 ;; *) echo "unknown stage" >&2; exit 2 ;; esac; shift 2 ;;
    *) echo "usage: check.sh [--stage quick|merge|release]" >&2; exit 2 ;;
  esac
done
cd "$(git rev-parse --show-toplevel)"
for dir in "$HOME/.cargo/bin" /opt/homebrew/bin /usr/local/bin; do
  [ -x "$dir/cargo" ] && PATH="$dir:$PATH" && break
done
step() { printf '\n==> %s\n' "$1"; }

step "Secret scan";  gitleaks git --redact --no-banner
step "Format";       cargo fmt --check
step "Lint";         cargo clippy --all-targets --locked -- -D warnings
step "Unit tests";   cargo test --locked
step "Build";        cargo build --release --locked
# No integration or end-to-end tests: the plugin needs a running Herdr server
# and signed-in Claude Code accounts. The release stage adds nothing beyond
# the release build above.
