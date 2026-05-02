import * as path from 'node:path';
import * as vscode from 'vscode';
import type { TweegoIntegration } from '../tweegoIntegration';

export function registerBuildCommands(
  context: vscode.ExtensionContext,
  getTweego: () => TweegoIntegration | undefined,
): void {

  // Build
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.build', async () => {
      const result = await getTweego()?.build(false);
      if (result?.success) {
        const outFile   = getOutputFile();
        const wsFolders = vscode.workspace.workspaceFolders;
        if (wsFolders?.[0]) {
          const absUri = getOutputUri(wsFolders[0].uri, outFile);
          vscode.window.showInformationMessage(
            `knot: Build succeeded → ${outFile}`, 'Open in browser',
          ).then(p => { if (p === 'Open in browser') vscode.env.openExternal(absUri); });
        }
      }
    }),
  );

  // Build test mode
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.buildTest', async () => {
      await getTweego()?.build(true);
    }),
  );

  // Start watch
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.startWatch', async () => {
      const tweego = getTweego();
      if (!tweego) return;
      await tweego.startWatch();
      const outFile   = getOutputFile();
      const wsFolders = vscode.workspace.workspaceFolders;
      if (wsFolders?.[0]) {
        const absUri = getOutputUri(wsFolders[0].uri, outFile);
        vscode.window.showInformationMessage(
          `knot: Watch mode started. Output: ${absUri.fsPath}`,
          'Open in browser', 'Stop watching',
        ).then(p => {
          if (p === 'Open in browser') vscode.env.openExternal(absUri);
          if (p === 'Stop watching')   vscode.commands.executeCommand('knot.stopWatch');
        });
      }
    }),
  );

  // Stop watch
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.stopWatch', () => {
      getTweego()?.stopWatch();
    }),
  );

  // List formats
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.listFormats', async () => {
      const tweego = getTweego();
      if (!tweego) return;
      const formats = await tweego.listFormats();
      if (!formats.length) {
        vscode.window.showInformationMessage(
          'knot: No story formats found. Check storyFormatsDirectory in knot Settings.',
          'Open Settings',
        ).then(p => {
          if (p === 'Open Settings') vscode.commands.executeCommand('knot.openSettings');
        });
        return;
      }
      await vscode.window.showQuickPick(
        formats.map(f => ({ label: f.id, description: `${f.name} ${f.version}` })),
        { placeHolder: 'Available story formats (read-only)' },
      );
    }),
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function getOutputFile(): string {
  return vscode.workspace.getConfiguration('knot.tweego')
    .get<string>('outputFile', 'dist/index.html');
}

function getOutputUri(wsFolderUri: vscode.Uri, outFile: string): vscode.Uri {
  return path.isAbsolute(outFile)
    ? vscode.Uri.file(outFile)
    : vscode.Uri.joinPath(wsFolderUri, outFile);
}