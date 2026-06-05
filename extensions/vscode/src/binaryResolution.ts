//! Binary resolution for the Knot language server.
//!
//! Handles platform detection and path resolution for the `knot-server`
//! binary, including user overrides and bundled fallbacks.

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';

/** Map VS Code platform to the Knot server binary name. */
export function getPlatformBinary(): string | null {
    const platform = process.platform;
    const arch = process.arch;

    if (platform === 'win32' && arch === 'x64') return 'knot-server.exe';
    if (platform === 'darwin' && arch === 'arm64') return 'knot-server';
    if (platform === 'darwin' && arch === 'x64') return 'knot-server';
    if (platform === 'linux' && arch === 'x64') return 'knot-server';
    if (platform === 'linux' && arch === 'arm64') return 'knot-server';

    return null;
}

/** Resolve the path to the knot-server binary. */
export async function getServerPath(context: vscode.ExtensionContext): Promise<string | null> {
    // Check user override first
    const configPath = vscode.workspace.getConfiguration('knot').get<string>('server.path');
    if (configPath && configPath.trim() !== '') {
        if (fs.existsSync(configPath)) {
            return configPath;
        }
        vscode.window.showWarningMessage(
            `Knot: Configured server path does not exist: ${configPath}`
        );
    }

    // Use bundled binary
    const binaryName = getPlatformBinary();
    if (!binaryName) {
        vscode.window.showWarningMessage(
            `Knot: No native binary available for ${process.platform}-${process.arch}. ` +
            'Falling back to TextMate grammar highlighting only.'
        );
        return null;
    }

    const serverPath = path.join(context.extensionPath, 'bin', binaryName);
    if (!fs.existsSync(serverPath)) {
        vscode.window.showWarningMessage(
            `Knot: Server binary not found at ${serverPath}. ` +
            'Falling back to TextMate grammar highlighting only.'
        );
        return null;
    }

    return serverPath;
}
