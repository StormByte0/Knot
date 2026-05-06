/**
 * Knot v2 — LSP Commands
 *
 * Implements all LSP-related commands declared in package.json:
 *   knot.showOutput
 *   knot.restart
 *   knot.refreshDocuments
 *   knot.goToPassage
 *   knot.openSettings
 *   knot.listFormats
 *   knot.selectFormat
 */

import * as vscode from 'vscode';
import { LanguageClient } from 'vscode-languageclient/node';
import { StatusBar } from '../statusBar';

export function registerLspCommands(
  context: vscode.ExtensionContext,
  client: LanguageClient,
  statusBar: StatusBar,
): void {
  // ─── knot.showOutput ───────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.showOutput', () => {
      client.outputChannel.show();
    }),
  );

  // ─── knot.restart ──────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.restart', async () => {
      statusBar.updateStatus('stopping');
      try {
        await client.stop();
        statusBar.updateStatus('starting');
        await client.start();
        statusBar.updateStatus('running');
        vscode.window.showInformationMessage('Knot: Language server restarted');
      } catch (err) {
        statusBar.updateStatus('error');
        vscode.window.showErrorMessage(`Knot: Failed to restart server: ${err}`);
      }
    }),
  );

  // ─── knot.refreshDocuments ─────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.refreshDocuments', async () => {
      try {
        await client.sendRequest('knot/refreshDocuments', {});
        vscode.window.showInformationMessage('Knot: Documents refreshed');
      } catch (err) {
        vscode.window.showErrorMessage(`Knot: Failed to refresh documents: ${err}`);
      }
    }),
  );

  // ─── knot.goToPassage ──────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.goToPassage', async () => {
      try {
        // Request passage list from server
        const passages = await client.sendRequest<string[]>('knot/listPassages');
        if (!passages || passages.length === 0) {
          vscode.window.showWarningMessage('Knot: No passages found in workspace');
          return;
        }

        const picked = await vscode.window.showQuickPick(passages, {
          placeHolder: 'Go to passage...',
        });

        if (picked) {
          // Request passage location from server and navigate
          const locations = await client.sendRequest<any[]>('textDocument/definition', {
            textDocument: { uri: vscode.window.activeTextEditor?.document.uri.toString() },
            position: { line: 0, character: 0 },
          });

          // Fallback: search in open documents
          const files = await vscode.workspace.findFiles('**/*.tw');
          const tweeFiles = files.concat(await vscode.workspace.findFiles('**/*.twee'));

          for (const fileUri of tweeFiles) {
            const doc = await vscode.workspace.openTextDocument(fileUri);
            const text = doc.getText();
            const headerRegex = new RegExp(`^::\\s*${escapeRegex(picked)}\\b`, 'm');
            const match = headerRegex.exec(text);
            if (match) {
              const position = doc.positionAt(match.index);
              const editor = await vscode.window.showTextDocument(doc, {
                selection: new vscode.Range(position, position),
              });
              editor.revealRange(
                new vscode.Range(position, position),
                vscode.TextEditorRevealType.InCenter,
              );
              return;
            }
          }

          vscode.window.showWarningMessage(`Knot: Passage "${picked}" not found in workspace files`);
        }
      } catch (err) {
        vscode.window.showErrorMessage(`Knot: Go to passage failed: ${err}`);
      }
    }),
  );

  // ─── knot.openSettings ─────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.openSettings', () => {
      vscode.commands.executeCommand('workbench.action.openSettings', 'knot.');
    }),
  );

  // ─── knot.listFormats ──────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.listFormats', async () => {
      try {
        const formats = await client.sendRequest<Array<{ id: string; name: string; version: string }>>(
          'knot/listFormats',
        );
        if (!formats || formats.length === 0) {
          vscode.window.showInformationMessage('Knot: No story formats registered');
          return;
        }

        const items = formats.map(f => ({
          label: f.name,
          description: `v${f.version}`,
          detail: `Format ID: ${f.id}`,
        }));

        await vscode.window.showQuickPick(items, {
          placeHolder: 'Available story formats',
        });
      } catch (err) {
        vscode.window.showErrorMessage(`Knot: Failed to list formats: ${err}`);
      }
    }),
  );

  // ─── knot.selectFormat ─────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.selectFormat', async () => {
      try {
        const formats = await client.sendRequest<Array<{ id: string; name: string; version: string }>>(
          'knot/listFormats',
        );
        if (!formats || formats.length === 0) {
          vscode.window.showWarningMessage('Knot: No story formats available');
          return;
        }

        const items = formats.map(f => ({
          label: f.name,
          description: `v${f.version}`,
          formatId: f.id,
        }));

        const picked = await vscode.window.showQuickPick(items, {
          placeHolder: 'Select active story format...',
        });

        if (picked) {
          // Send format selection to server
          const result = await client.sendRequest<{ success: boolean; formatName: string }>(
            'knot/selectFormat',
            { formatId: picked.formatId },
          );

          if (result.success) {
            // Update configuration
            const config = vscode.workspace.getConfiguration('knot');
            await config.update('format.activeFormat', picked.formatId, vscode.ConfigurationTarget.Workspace);

            // Update status bar
            statusBar.updateFormat(picked.formatId, picked.label);

            vscode.window.showInformationMessage(`Knot: Active format set to ${picked.label}`);
          }
        }
      } catch (err) {
        vscode.window.showErrorMessage(`Knot: Failed to select format: ${err}`);
      }
    }),
  );
}

// ─── Utility ─────────────────────────────────────────────────

function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
