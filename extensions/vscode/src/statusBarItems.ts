//! Status bar items for the Knot extension (left side).
//!
//! Creates a compact group of status bar items on the left side:
//! - Story Map launch button
//! - Build button
//! - Watch toggle (auto-rebuild on save)
//! - Play button (open compiled HTML in browser)
//! - Extension settings (cog)
//!
//! The indexing progress item (managed by extension.ts) shows during
//! startup, then these items take over as the permanent left-side
//! Knot status cluster. The right side is left untouched for VS Code's
//! language mode indicator and future extensions (formatter, etc.).

import * as vscode from 'vscode';
import { isWatchActive, toggleWatch, setBuildOutputChannel } from './watchState';

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export interface KnotStatusBarItems {
    storyMap: vscode.StatusBarItem;
    build: vscode.StatusBarItem;
    watch: vscode.StatusBarItem;
    play: vscode.StatusBarItem;
    settings: vscode.StatusBarItem;
    dispose: () => void;
}

/**
 * Create and register the Knot status bar items on the LEFT side.
 *
 * Items use descending priorities so they appear in order:
 * [Story Map] [Build] [Watch] [Play] [⚙]
 *
 * Priority 50 is used by the indexing progress item. We use 49-45
 * so our items appear just to the right of the indexing item (which
 * is hidden after indexing completes).
 */
export function createStatusBarItems(
    context: vscode.ExtensionContext,
    buildOutputChannel: vscode.OutputChannel,
    getClient: () => unknown,
): KnotStatusBarItems {
    // Wire the build output channel into the watch state module so the
    // watcher can log build progress.
    setBuildOutputChannel(buildOutputChannel);

    const storyMap = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        49,
    );
    const build = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        48,
    );
    const watch = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        47,
    );
    const play = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        46,
    );
    const settings = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        45,
    );

    // ── Story Map ────────────────────────────────────────────────────
    storyMap.text = '$(compass) Story Map';
    storyMap.tooltip = 'Knot: Open the Story Map';
    storyMap.command = 'knot.openStoryMap';
    storyMap.show();

    // ── Build ────────────────────────────────────────────────────────
    build.text = '$(tools) Build';
    build.tooltip = 'Knot: Build the project (F6)';
    build.command = 'knot.build';
    build.show();

    // ── Watch ────────────────────────────────────────────────────────
    // Toggle icon between eye (active) and eye-closed (inactive).
    const updateWatchIcon = () => {
        if (isWatchActive()) {
            watch.text = '$(eye) Watch';
            watch.tooltip = 'Knot: Watch is ON — auto-rebuild on save. Click to stop.';
        } else {
            watch.text = '$(eye-closed) Watch';
            watch.tooltip = 'Knot: Watch is OFF. Click to auto-rebuild on save.';
        }
    };
    watch.command = 'knot.toggleWatch';
    updateWatchIcon();
    // Register the watch toggle command here (not in commands.ts) because
    // this is the only place that owns the watch icon — registering in
    // both places would cause one to silently override the other.
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.toggleWatch', async () => {
            const client = getClient() as { isRunning?: () => boolean } | null;
            // toggleWatch expects a KnotLanguageClient; cast through unknown
            toggleWatch(client as never);
            updateWatchIcon();
            vscode.window.showInformationMessage(
                `Knot: Watch ${isWatchActive() ? 'enabled' : 'disabled'}.`,
            );
        }),
    );
    watch.show();

    // ── Play ─────────────────────────────────────────────────────────
    play.text = '$(play) Play';
    play.tooltip = 'Knot: Open compiled HTML in browser (F5)';
    play.command = 'knot.play';
    play.show();

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
    context.subscriptions.push(storyMap, build, watch, play, settings);

    return {
        storyMap,
        build,
        watch,
        play,
        settings,
        dispose: () => {
            storyMap.dispose();
            build.dispose();
            watch.dispose();
            play.dispose();
            settings.dispose();
            settingsCommand.dispose();
        },
    };
}
