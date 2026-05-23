#!/usr/bin/env bash
set -euo pipefail

echo "=== Knot Package Script ==="
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Run build first
"$PROJECT_ROOT/scripts/build.sh"

# Package as VSIX
echo ""
echo "Packaging VSIX..."
cd "$PROJECT_ROOT/extensions/vscode"
npx vsce package --allow-missing-repository

echo ""
echo "=== Package complete ==="
ls -la *.vsix 2>/dev/null || echo "No VSIX file found"
