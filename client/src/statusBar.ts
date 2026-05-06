/**
 * Knot v2 — Status Bar
 *
 * Displays server connection status and active story format
 * in the VS Code status bar.
 */

import * as vscode from 'vscode';
import { LanguageClient } from 'vscode-languageclient/node';

export class StatusBar implements vscode.Disposable {
  private statusItem: vscode.StatusBarItem;
  private formatItem: vscode.StatusBarItem;
  private client: LanguageClient;

  constructor(client: LanguageClient) {
    this.client = client;

    // Server status (left side)
    this.statusItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      50,
    );
    this.statusItem.command = 'knot.mainMenu';
    this.updateStatus('running');

    // Active format (right side)
    this.formatItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Right,
      50,
    );
    this.formatItem.command = 'knot.selectFormat';
    this.refreshFromConfig();

    // Show items
    this.statusItem.show();
    this.formatItem.show();
  }

  /** Update the server connection status display. */
  updateStatus(status: 'starting' | 'running' | 'stopping' | 'error' | 'idle'): void {
    switch (status) {
      case 'starting':
        this.statusItem.text = '$(sync~spin) Knot';
        this.statusItem.tooltip = 'Knot: Starting language server...';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'running':
        this.statusItem.text = '$(check) Knot';
        this.statusItem.tooltip = 'Knot: Language server running';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'stopping':
        this.statusItem.text = '$(sync~spin) Knot';
        this.statusItem.tooltip = 'Knot: Stopping language server...';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'error':
        this.statusItem.text = '$(error) Knot';
        this.statusItem.tooltip = 'Knot: Language server error';
        this.statusItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
        break;
      case 'idle':
        this.statusItem.text = '$(circle-slash) Knot';
        this.statusItem.tooltip = 'Knot: Idle';
        this.statusItem.backgroundColor = undefined;
        break;
    }
  }

  /** Update the active story format display. */
  updateFormat(formatId: string, formatName: string): void {
    this.formatItem.text = `$(bookmark) ${formatName}`;
    this.formatItem.tooltip = `Knot: Active format is ${formatName} (${formatId})\nClick to change`;
  }

  /** Refresh format display from configuration. */
  refreshFromConfig(): void {
    const config = vscode.workspace.getConfiguration('knot');
    const formatId = config.get<string>('format.activeFormat', '');
    if (formatId) {
      // Map known format IDs to display names
      const nameMap: Record<string, string> = {
        'sugarcube-2': 'SugarCube 2',
        'harlowe-3': 'Harlowe 3',
        'chapbook-1': 'Chapbook 1',
        'fallback': 'Basic Twee',
      };
      this.updateFormat(formatId, nameMap[formatId] ?? formatId);
    } else {
      this.updateFormat('auto', 'Auto-detect');
    }
  }

  dispose(): void {
    this.statusItem.dispose();
    this.formatItem.dispose();
  }
}
