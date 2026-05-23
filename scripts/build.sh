#!/usr/bin/env bash
set -euo pipefail

echo "=== Knot Build Script ==="
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Step 1: Build Rust server
echo ""
echo "[1/4] Building Rust LSP server..."
cd "$PROJECT_ROOT"
cargo build --release --manifest-path crates/server/Cargo.toml
mkdir -p extensions/vscode/bin
cp target/release/knot-server extensions/vscode/bin/ 2>/dev/null || \
cp target/release/knot-server.exe extensions/vscode/bin/ 2>/dev/null || \
echo "WARNING: Could not find knot-server binary"

# Step 2: Install webview dependencies
echo ""
echo "[2/4] Installing webview dependencies..."
cd "$PROJECT_ROOT/extensions/vscode/webview"
npm install

# Step 3: Build React webview
echo ""
echo "[3/4] Building React webview..."
npm run build

# Step 4: Install extension dependencies and compile
echo ""
echo "[4/4] Compiling VS Code extension..."
cd "$PROJECT_ROOT/extensions/vscode"
npm install
npm run compile

echo ""
echo "=== Build complete ==="
echo "Extension output: extensions/vscode/out/"
echo "Webview output:   extensions/vscode/media/storymap/"
echo "Server binary:    extensions/vscode/bin/knot-server"
echo ""
echo "To package as VSIX: cd extensions/vscode && npx vsce package"
