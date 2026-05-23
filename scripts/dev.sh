#!/usr/bin/env bash
set -euo pipefail

echo "=== Knot Dev Mode ==="
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Build Rust server in debug mode
echo "[1/3] Building Rust server (debug)..."
cd "$PROJECT_ROOT"
cargo build --manifest-path crates/server/Cargo.toml
mkdir -p extensions/vscode/bin
cp target/debug/knot-server extensions/vscode/bin/ 2>/dev/null || \
cp target/debug/knot-server.exe extensions/vscode/bin/ 2>/dev/null || \
echo "WARNING: Could not find knot-server binary"

# Build webview in watch mode
echo ""
echo "[2/3] Starting webview dev server..."
cd "$PROJECT_ROOT/extensions/vscode/webview"
npm install
npx vite build --watch &
VITE_PID=$!

# Compile extension in watch mode
echo ""
echo "[3/3] Starting extension compile watcher..."
cd "$PROJECT_ROOT/extensions/vscode"
npm install
npx tsc -watch &

echo ""
echo "Dev servers running. Press Ctrl+C to stop."
trap "kill $VITE_PID 2>/dev/null; wait" EXIT
wait
