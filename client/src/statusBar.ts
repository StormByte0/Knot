/**
 * Knot v2 — Status Bar
 *
 * Displays server connection status, active story format,
 * passage count, and diagnostics count in the VS Code status bar.
 * Every update is logged for debugging.
 */

import * as vscode from 'vscode';
import { LanguageClient, State } from 'vscode-languageclient/node';
import { LogManager } from './logManager';

export class StatusBar implements vscode.Disposable {
  private statusItem: vscode.StatusBarItem;
  private formatItem: vscode.StatusBarItem;
  private infoItem: vscode.StatusBarItem;
  private client: LanguageClient;

  // Cached state for the Show Status command
  private _currentFormat: { id: string; name: string } = { id: 'auto', name: 'Auto-detect' };
  private _serverState: string = 'starting';
  private _lastError: string | undefined;

  constructor(client: LanguageClient) {
    this.client = client;
    const log = LogManager.instance;

    // Server status (left side)
    this.statusItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      50,
    );
    this.statusItem.command = 'knot.showOutput';
    this.updateStatus('starting');
    log.debug('Status bar: status item created');

    // Active format (left side, after status)
    this.formatItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      49,
    );
    this.formatItem.command = 'knot.selectFormat';
    this.refreshFromConfig();
    log.debug('Status bar: format item created');

    // Info line (passages count, diagnostics)
    this.infoItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      48,
    );
    this.infoItem.command = 'knot.showStatus';
    this.infoItem.text = '$(file) --';
    this.infoItem.tooltip = 'Knot: No workspace info yet';
    log.debug('Status bar: info item created');

    // Show items
    this.statusItem.show();
    this.formatItem.show();
    this.infoItem.show();

    log.info('Status bar initialized');

    // Start periodic info refresh
    this.startInfoRefresh();
  }

  /** Update the server connection status display. */
  updateStatus(status: 'starting' | 'running' | 'stopping' | 'error' | 'idle'): void {
    this._serverState = status;
    const log = LogManager.instance;

    switch (status) {
      case 'starting':
        this.statusItem.text = '$(sync~spin) Knot';
        this.statusItem.tooltip = 'Knot: Starting language server...';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'running':
        this.statusItem.text = '$(check) Knot';
        this.statusItem.tooltip = 'Knot: Language server running\nClick to show output';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'stopping':
        this.statusItem.text = '$(sync~spin) Knot';
        this.statusItem.tooltip = 'Knot: Stopping language server...';
        this.statusItem.backgroundColor = undefined;
        break;
      case 'error':
        this.statusItem.text = '$(error) Knot';
        this.statusItem.tooltip = 'Knot: Language server error\nClick to show output';
        this.statusItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
        break;
      case 'idle':
        this.statusItem.text = '$(circle-slash) Knot';
        this.statusItem.tooltip = 'Knot: Server stopped\nClick to show output';
        this.statusItem.backgroundColor = undefined;
        break;
    }

    log.debug(`Status bar: status → ${status}`);
  }

  /** Record an error for the Show Status command. */
  setLastError(message: string): void {
    this._lastError = message;
  }

  /** Update the active story format display. */
  updateFormat(formatId: string, formatName: string): void {
    this._currentFormat = { id: formatId, name: formatName };
    this.formatItem.text = `$(bookmark) ${formatName}`;
    this.formatItem.tooltip = `Knot: Active format is ${formatName} (${formatId})\nClick to change`;
    LogManager.instance.debug(`Status bar: format → ${formatName} (${formatId})`);
  }

  /** Update passage count immediately (from server status notification). */
  updatePassageCount(count: number): void {
    // Get current diagnostic count
    let diagCount = 0;
    for (const editor of vscode.window.visibleTextEditors) {
      const diags = vscode.languages.getDiagnostics(editor.document.uri);
      const knotDiags = diags.filter(d => d.source === 'knot');
      diagCount += knotDiags.length;
    }
    const diagIcon = diagCount > 0 ? `$(warning) ${diagCount}` : '$(check) 0';
    this.infoItem.text = `$(file) ${count} passages | ${diagIcon} issues`;
    this.infoItem.tooltip = `Knot: ${count} passages indexed, ${diagCount} diagnostics\nClick for status details`;
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
        'chapbook-2': 'Chapbook 2',
        'chapbook-1': 'Chapbook 1',
        'snowman-2': 'Snowman 2',
        'fallback': 'Basic Twee',
      };
      this.updateFormat(formatId, nameMap[formatId] ?? formatId);
    } else {
      this.updateFormat('auto', 'Auto-detect');
    }
  }

  /** Get current status info for the Show Status command. */
  getStatusInfo(): {
    serverState: string;
    format: { id: string; name: string };
    lastError: string | undefined;
  } {
    return {
      serverState: this._serverState,
      format: this._currentFormat,
      lastError: this._lastError,
    };
  }

  dispose(): void {
    this.statusItem.dispose();
    this.formatItem.dispose();
    this.infoItem.dispose();
  }

  // ─── Private ──────────────────────────────────────────────────

  private _refreshInterval: ReturnType<typeof setInterval> | undefined;

  private startInfoRefresh(): void {
    // Refresh passage/diagnostic info every 5 seconds
    this._refreshInterval = setInterval(() => {
      this.refreshInfo();
    }, 5000);

    // Also refresh immediately
    this.refreshInfo();
  }

  private async refreshInfo(): Promise<void> {
    try {
      if (!this.client || this.client.state !== State.Running) {
        this.infoItem.text = '$(file) --';
        this.infoItem.tooltip = 'Knot: Server not running';
        return;
      }

      // Request passage count from server
      // Pass null as params — JSON-RPC requires params even for void-typed requests
      const passages = await this.client.sendRequest<string[]>('knot/listPassages', null).catch(() => []);
      const passageCount = passages?.length ?? 0;

      // Get diagnostic count from visible editors
      let diagCount = 0;
      for (const editor of vscode.window.visibleTextEditors) {
        const diags = vscode.languages.getDiagnostics(editor.document.uri);
        const knotDiags = diags.filter(d => d.source === 'knot');
        diagCount += knotDiags.length;
      }

      const diagIcon = diagCount > 0 ? `$(warning) ${diagCount}` : '$(check) 0';
      this.infoItem.text = `$(file) ${passageCount} passages | ${diagIcon} issues`;
      this.infoItem.tooltip = `Knot: ${passageCount} passages indexed, ${diagCount} diagnostics\nClick for status details`;
    } catch {
      // Silently fail — status bar refresh is non-critical
      this.infoItem.text = '$(file) --';
    }
  }
}
