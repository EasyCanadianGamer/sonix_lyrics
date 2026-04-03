#!/usr/bin/env bash

set -e

# Source Cargo if not already on PATH
if ! command -v cargo &>/dev/null; then
    source "$HOME/.cargo/env"
fi

echo "==> Checking formatting..."
cargo fmt --check || { echo "FAIL: Run 'cargo fmt' to fix formatting"; exit 1; }

echo "==> Running clippy..."
cargo clippy -- -D warnings 2>&1 | grep -v "^warning: unused imports\|^warning: unused variable" || true

echo "==> Building (debug)..."
cargo build

echo ""
echo "Build OK. Launching TUI (Ctrl+C or 'q' to exit)..."
echo ""
cargo run
