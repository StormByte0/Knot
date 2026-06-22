//! Status bar items for the Knot extension (left side).
//!
//! Creates a compact group of status bar items on the left side:
//! - Story Map launch button
//! - Build button
//! - Extension settings (cog)
//!
//! The indexing progress item (managed by extension.ts) shows during
//! startup, then these items take over as the permanent left-side
//! Knot status cluster. The right side is left untouched for VS Code's
//! language mode indicator and future extensions (formatter, etc.).

import * as vscode from 'vscode';

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export interface KnotStatusBarItems {
    storyMap: vscode.StatusBarItem;
    build: vscode.StatusBarItem;
    settings: vscode.StatusBarItem;
    dispose: () => void;
}

/**
 * Create and register the Knot status bar items on the LEFT side.
 *
 * Items use descending priorities so they appear in order:
 * [Story Map] [Build] [⚙]
 *
 * Priority 50 is used by the indexing progress item. We use 49-47
 * so our items appear just to the right of the indexing item (which
 * is hidden after indexing completes).
 */
export function createStatusBarItems(
    context: vscode.ExtensionContext,
): KnotStatusBarItems {
    const storyMap = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        49,
    );
    const build = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        48,
    );
    const settings = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        47,
    );

    // ── Story Map ────────────────────────────────────────────────────
    storyMap.text = '$(graph) Story Map';
    storyMap.tooltip = 'Knot: Open the Story Map';
    storyMap.command = 'knot.openStoryMap';
    storyMap.show();

    // ── Build ────────────────────────────────────────────────────────
    build.text = '$(play) Build';
    build.tooltip = 'Knot: Build the project';
    build.command = 'knot.build';
    build.show();

    // ── Settings ─────────────────────────────────────────────────────
    settings.text = '$(gear)';
    settings.tooltip = 'Knot: Extension Settings';
    settings.command = 'knot.openSettings';
    settings.show();

    // Register the settings wrapper command (opens settings filtered to Knot)
    const settingsCommand = vscode.commands.registerCommand('knot.openSettings', () => {
        vscode.commands.executeCommand('workbench.action.openSettings', '@ext:stormbyte.knot');
    });
    context.subscriptions.push(settingsCommand);

    // Push items to subscriptions for cleanup
    context.subscriptions.push(storyMap, build, settings);

    return {
        storyMap,
        build,
        settings,
        dispose: () => {
            storyMap.dispose();
            build.dispose();
            settings.dispose();
            settingsCommand.dispose();
        },
    };
}
