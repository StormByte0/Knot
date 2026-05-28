//! Sidebar launch card for the Story Map.
//!
//! This implements a TreeDataProvider that provides an always-visible
//! "Open Story Map" action via the view title bar icon. When the tree
//! is empty (normal state), VS Code shows the welcome content which
//! includes a clickable button to launch the graph.
//!
//! The view title action (icon in the section header) is ALWAYS visible
//! even when the sidebar section is collapsed — this solves the
//! discoverability problem of hiding the launch button inside a
//! collapsible webview tab.

import * as vscode from 'vscode';

export class StoryMapLaunchProvider implements vscode.TreeDataProvider<never> {
    public static readonly viewType = 'knot.storyMapLaunch';

    private _onDidChangeTreeData = new vscode.EventEmitter<never | undefined | null>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    constructor(private readonly _extensionUri: vscode.Uri) {}

    getTreeItem(_element: never): vscode.TreeItem {
        // This provider always returns an empty tree; the real UI
        // is the view title action and the welcome content.
        throw new Error('StoryMapLaunchProvider has no tree items');
    }

    getChildren(_element?: never): never[] {
        // Always empty — the view shows welcome content instead
        return [];
    }

    refresh(): void {
        this._onDidChangeTreeData.fire(undefined);
    }
}
