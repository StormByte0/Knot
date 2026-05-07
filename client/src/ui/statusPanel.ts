/**
 * Knot v2 — Status Panel
 *
 * Provides a "Knot: Show Status" command that displays a comprehensive
 * health check of the extension: server state, format, passages,
 * diagnostics, configuration, and troubleshooting tips.
 *
 * This is THE debugging tool for when the extension seems "dead" —
 * it surfaces all internal state without needing a debugger.
 */

import * as vscode from 'vscode';
import { LanguageClient, State } from 'vscode-languageclient/node';
import { StatusBar } from '../statusBar';
import { LogManager } from '../logManager';

export class StatusPanel {
  private client: LanguageClient;
  private statusBar: StatusBar;

  constructor(client: LanguageClient, statusBar: StatusBar) {
    this.client = client;
    this.statusBar = statusBar;
  }

  /** Show the full status panel. */
  async show(): Promise<void> {
    const log = LogManager.instance;
    log.info('Showing status panel');

    const lines: string[] = [];

    // ─── Header ──────────────────────────────────────────────────
    lines.push('# Knot — Extension Status');
    lines.push('');
    lines.push(`**Generated:** ${new Date().toISOString()}`);
    lines.push('');

    // ─── Server ──────────────────────────────────────────────────
    const stateLabels: Record<number, string> = {
      [State.Running]: '🟢 Running',
      [State.Starting]: '🟡 Starting',
      [State.Stopped]: '🔴 Stopped',
    };
    const serverState = stateLabels[this.client.state] ?? `Unknown (${this.client.state})`;
    lines.push('## Server');
    lines.push('');
    lines.push(`- **State:** ${serverState}`);
    lines.push(`- **Client ID:** ${this.client.clientOptions.documentSelector ?? '(none)'}`);

    const statusInfo = this.statusBar.getStatusInfo();
    if (statusInfo.lastError) {
      lines.push(`- **Last Error:** ${statusInfo.lastError}`);
    }
    lines.push('');

    // ─── Format ──────────────────────────────────────────────────
    lines.push('## Story Format');
    lines.push('');
    lines.push(`- **Active Format:** ${statusInfo.format.name} (${statusInfo.format.id})`);
    lines.push('');

    // Try to get format list from server
    try {
      if (this.client.state === State.Running) {
        // Pass null as params — JSON-RPC requires params even for void-typed requests
        const formats = await this.client.sendRequest<Array<{ id: string; name: string; version: string }>>(
          'knot/listFormats',
          null,
        );
        if (formats && formats.length > 0) {
          lines.push('- **Available Formats:**');
          for (const f of formats) {
            const marker = f.id === statusInfo.format.id ? ' ← active' : '';
            lines.push(`  - ${f.name} v${f.version} (${f.id})${marker}`);
          }
        }
      } else {
        lines.push('- *Server not running — cannot query formats*');
      }
    } catch (err) {
      lines.push(`- *Error querying formats: ${err}*`);
    }
    lines.push('');

    // ─── Passages ────────────────────────────────────────────────
    lines.push('## Passages');
    lines.push('');
    try {
      if (this.client.state === State.Running) {
        // Pass null as params — JSON-RPC requires params even for void-typed requests
        const passages = await this.client.sendRequest<string[]>('knot/listPassages', null);
        if (passages && passages.length > 0) {
          lines.push(`- **Total Passages:** ${passages.length}`);
          lines.push('- **Passage List:**');
          for (const name of passages.slice(0, 30)) {
            lines.push(`  - ${name}`);
          }
          if (passages.length > 30) {
            lines.push(`  - ... and ${passages.length - 30} more`);
          }
        } else {
          lines.push('- **No passages found** — Check if your .tw/.twee files have `::` passage headers');
        }
      } else {
        lines.push('- *Server not running — cannot query passages*');
      }
    } catch (err) {
      lines.push(`- *Error querying passages: ${err}*`);
    }
    lines.push('');

    // ─── Diagnostics ─────────────────────────────────────────────
    lines.push('## Diagnostics');
    lines.push('');
    let totalDiags = 0;
    let errorCount = 0;
    let warnCount = 0;
    for (const editor of vscode.window.visibleTextEditors) {
      const diags = vscode.languages.getDiagnostics(editor.document.uri);
      const knotDiags = diags.filter(d => d.source === 'knot');
      totalDiags += knotDiags.length;
      errorCount += knotDiags.filter(d => d.severity === vscode.DiagnosticSeverity.Error).length;
      warnCount += knotDiags.filter(d => d.severity === vscode.DiagnosticSeverity.Warning).length;
    }
    lines.push(`- **Total Issues:** ${totalDiags}`);
    lines.push(`- **Errors:** ${errorCount}`);
    lines.push(`- **Warnings:** ${warnCount}`);
    if (totalDiags === 0) {
      lines.push('- *No diagnostics published — this may mean the server is not running or format detection failed*');
    }
    lines.push('');

    // ─── Configuration ───────────────────────────────────────────
    lines.push('## Configuration');
    lines.push('');
    const config = vscode.workspace.getConfiguration('knot');
    const configKeys = [
      'format.activeFormat',
      'format.formatsDirectory',
      'tweego.path',
      'tweego.outputFile',
      'lint.unknownPassage',
      'lint.unknownMacro',
      'lint.duplicatePassage',
      'lint.typeMismatch',
      'lint.unreachablePassage',
      'lint.containerStructure',
      'lint.deprecatedMacro',
    ];
    for (const key of configKeys) {
      const value = config.get(key, '(not set)');
      lines.push(`- **knot.${key}:** ${value}`);
    }
    lines.push('');

    // ─── Workspace ───────────────────────────────────────────────
    lines.push('## Workspace');
    lines.push('');
    const folders = vscode.workspace.workspaceFolders;
    lines.push(`- **Workspace Folders:** ${folders ? folders.map(f => f.uri.fsPath).join(', ') : '(none)'}`);

    const activeEditor = vscode.window.activeTextEditor;
    if (activeEditor) {
      lines.push(`- **Active Document:** ${activeEditor.document.uri.fsPath}`);
      lines.push(`- **Active Language:** ${activeEditor.document.languageId}`);
      lines.push(`- **Line Count:** ${activeEditor.document.lineCount}`);
    } else {
      lines.push('- **Active Document:** (none)');
    }
    lines.push('');

    // ─── Open Documents ──────────────────────────────────────────
    const tweeDocs = vscode.workspace.textDocuments.filter(
      d => d.languageId === 'twine' || d.fileName.endsWith('.tw') || d.fileName.endsWith('.twee'),
    );
    lines.push(`- **Open Twee Documents:** ${tweeDocs.length}`);
    for (const doc of tweeDocs.slice(0, 10)) {
      lines.push(`  - ${doc.uri.fsPath} (${doc.languageId}, ${doc.lineCount} lines)`);
    }
    lines.push('');

    // ─── Troubleshooting ─────────────────────────────────────────
    lines.push('## Troubleshooting');
    lines.push('');

    if (this.client.state !== State.Running) {
      lines.push('⚠️ **Server is not running.** Try:');
      lines.push('- Run "Knot: Restart Language Server"');
      lines.push('- Check Output → "Knot" for error messages');
      lines.push('- Verify the server module exists in the extension directory');
      lines.push('');
    }

    if (tweeDocs.length === 0) {
      lines.push('⚠️ **No Twee documents are open.** The extension only activates features for .tw/.twee files.');
      lines.push('- Open a .tw or .twee file');
      lines.push('- Make sure the file language is set to "Twine" (bottom-right status bar)');
      lines.push('');
    }

    if (statusInfo.format.id === 'fallback') {
      lines.push('⚠️ **Fallback format is active.** This means:');
      lines.push('- No StoryData passage was found (or format auto-detection failed)');
      lines.push('- Only basic [[link]] support is available');
      lines.push('- Run "Knot: Select Story Format" to manually choose a format');
      lines.push('- Check that your StoryData passage has a valid JSON `format` field');
      lines.push('');
    }

    if (totalDiags === 0 && this.client.state === State.Running && tweeDocs.length > 0) {
      lines.push('ℹ️ **No diagnostics published.** Possible causes:');
      lines.push('- Format detection may have failed (check format above)');
      lines.push('- The server may not be receiving document sync events');
      lines.push('- Open Output → "Knot" and look for errors after editing a file');
      lines.push('');
    }

    // ─── Display ─────────────────────────────────────────────────
    const content = lines.join('\n');

    // Show in a webview panel for nice formatting
    const panel = vscode.window.createWebviewPanel(
      'knotStatus',
      'Knot Status',
      vscode.ViewColumn.One,
      { enableScripts: false },
    );

    panel.webview.html = this.renderHtml(content);
  }

  private renderHtml(markdown: string): string {
    // Simple markdown-to-HTML conversion for the status panel
    let html = markdown
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      // Headers
      .replace(/^# (.+)$/gm, '<h1>$1</h1>')
      .replace(/^## (.+)$/gm, '<h2>$1</h2>')
      // Bold
      .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
      // Italic
      .replace(/\*(.+?)\*/g, '<em>$1</em>')
      // List items
      .replace(/^- /gm, '<li>')
      .replace(/^  - /gm, '<li style="margin-left: 1.5em">')
      // Line breaks
      .replace(/\n\n/g, '<br><br>')
      .replace(/\n/g, '<br>');

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>Knot Status</title>
  <style>
    body {
      font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 1.5em;
      max-width: 800px;
      margin: 0 auto;
      line-height: 1.6;
    }
    h1 { color: var(--vscode-editor-foreground); border-bottom: 1px solid var(--vscode-widget-border, #444); padding-bottom: 0.3em; }
    h2 { color: var(--vscode-editor-foreground); margin-top: 1.5em; }
    li { margin: 0.2em 0; }
    code { background: var(--vscode-textCodeBlock-background, rgba(255,255,255,0.05)); padding: 0.15em 0.4em; border-radius: 3px; font-size: 0.9em; }
    strong { color: var(--vscode-editor-foreground); }
  </style>
</head>
<body>
${html}
</body>
</html>`;
  }
}
