import * as cp from 'node:child_process';
import * as vscode from 'vscode';
import type { LanguageClient } from 'vscode-languageclient/node';

interface PassageEntry {
  name:         string;
  uri:          string;
  fileName:     string;
  refCount:     number;
  incomingFrom: string[];
}
const GET_PASSAGES_REQUEST = 'knot/getPassages';

export function registerLspCommands(
  context: vscode.ExtensionContext,
  outputChannel: vscode.OutputChannel,
  getClient: () => LanguageClient | undefined,
  restartClient: () => Promise<void>,
): void {

  // ── Main menu ─────────────────────────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.mainMenu', async () => {
      const picked = await vscode.window.showQuickPick([
        { label: '$(symbol-module) Go to passage',           description: 'Ctrl+Alt+P',   cmd: 'knot.goToPassage' },
        { label: '$(refresh) Refresh workspace index',       description: '',              cmd: 'knot.refreshDocuments' },
        { label: '$(play) Build',                            description: 'Ctrl+Shift+B', cmd: 'knot.build' },
        { label: '$(beaker) Build (test mode)',              description: '',              cmd: 'knot.buildTest' },
        { label: '$(eye) Start watch mode',                  description: '',              cmd: 'knot.startWatch' },
        { label: '$(eye-closed) Stop watch mode',            description: '',              cmd: 'knot.stopWatch' },
        { label: '$(check) Verify Tweego',                   description: '',              cmd: 'knot.verifyTweego' },
        { label: '$(list-unordered) List story formats',     description: '',              cmd: 'knot.listFormats' },
        { label: '$(debug-restart) Restart language server', description: '',              cmd: 'knot.restart' },
        { label: '$(output) Show output',                    description: '',              cmd: 'knot.showOutput' },
        { label: '$(settings-gear) knot Settings',            description: '',              cmd: 'knot.openSettings' },
      ], { placeHolder: 'knot Language Server', matchOnDescription: true });
      if (picked) vscode.commands.executeCommand(picked.cmd);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('knot.showOutput', () => outputChannel.show()),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('knot.restart', async () => restartClient()),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('knot.refreshDocuments', async () => {
      const cfg = vscode.workspace.getConfiguration('knot.project');
      await getClient()?.sendNotification('knot/refreshDocuments', {
        exclude: cfg.get<string[]>('exclude', []),
      });
    }),
  );

  // ── Open native VS Code settings filtered to this extension ───────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.openSettings', () => {
      vscode.commands.executeCommand(
        'workbench.action.openSettings',
        '@ext:Stormbyte.knot',
      );
    }),
  );

  // ── Verify Tweego binary ───────────────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.verifyTweego', async () => {
      const cfg      = vscode.workspace.getConfiguration('knot.tweego');
      const execPath = cfg.get<string>('path', 'tweego').trim() || 'tweego';

      await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'knot: Verifying Tweego…' },
        async () => {
          const result = await runVerify(execPath);
          if (result.ok) {
            vscode.window.showInformationMessage(
              `knot: Tweego v${result.version} found at "${execPath}".`,
            );
          } else {
            vscode.window.showErrorMessage(
              `knot: Tweego not found or failed at "${execPath}". ${result.detail}`,
              'Configure path',
            ).then(p => {
              if (p === 'Configure path') {
                vscode.commands.executeCommand(
                  'workbench.action.openSettings',
                  'knot.tweego.path',
                );
              }
            });
          }
        },
      );
    }),
  );

  // ── Go to passage ─────────────────────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.goToPassage', async () => {
      const client = getClient();
      if (!client) {
        vscode.window.showWarningMessage('knot Language Server is not running.');
        return;
      }

      let entries: PassageEntry[];
      try {
        entries = await client.sendRequest<PassageEntry[]>(GET_PASSAGES_REQUEST);
      } catch {
        vscode.window.showErrorMessage('knot: Could not retrieve passage list. Is the server ready?');
        return;
      }
      if (!entries?.length) {
        vscode.window.showInformationMessage('knot: No passages found.');
        return;
      }

      const picked = await vscode.window.showQuickPick(
        entries.map(e => ({
          label:       e.name,
          description: e.fileName,
          detail:      e.incomingFrom.length > 0
            ? `← ${e.incomingFrom.join(', ')}`
            : '↚ no incoming links',
          entry: e,
        })),
        { placeHolder: 'Type to filter passages…', matchOnDescription: true, matchOnDetail: true },
      );
      if (!picked) return;

      const doc    = await vscode.workspace.openTextDocument(vscode.Uri.parse(picked.entry.uri));
      const editor = await vscode.window.showTextDocument(doc);
      const lines  = doc.getText().split('\n');
      let   target = 0;

      for (let i = 0; i < lines.length; i++) {
        if (/^::[ \t]/.test(lines[i]!)) {
          const m = lines[i]!.match(/^::[ \t]+([^\[{]+?)[ \t]*(?:[\[{]|$)/);
          if (m && m[1]!.trim() === picked.entry.name) { target = i; break; }
        }
      }

      const pos = new vscode.Position(target, 0);
      editor.selection = new vscode.Selection(pos, pos);
      editor.revealRange(new vscode.Range(pos, pos), vscode.TextEditorRevealType.AtTop);
    }),
  );
}

// ---------------------------------------------------------------------------
// Tweego verification — runs `tweego -v`, captures output
// ---------------------------------------------------------------------------

function runVerify(execPath: string): Promise<{ ok: boolean; version: string; detail: string }> {
  return new Promise(resolve => {
    const proc = cp.spawn(execPath, ['-v'], { timeout: 8_000 });
    let out = '';

    proc.stdout?.on('data', (d: Buffer) => { out += d.toString(); });
    proc.stderr?.on('data', (d: Buffer) => { out += d.toString(); });

    proc.on('error', (err: NodeJS.ErrnoException) => {
      if (err.code === 'ENOENT') {
        resolve({ ok: false, version: '', detail: 'Executable not found.' });
      } else {
        resolve({ ok: false, version: '', detail: err.message });
      }
    });

    proc.on('close', () => {
      const combined = out.trim();
      const match    = combined.match(/\bv?(\d+\.\d+(?:\.\d+)?)\b/);
      if (match) {
        resolve({ ok: true, version: match[1]!, detail: combined });
      } else if (combined.length > 0) {
        // Binary ran but output didn't look like a version — still counts as found
        resolve({ ok: true, version: 'unknown', detail: combined });
      } else {
        resolve({ ok: false, version: '', detail: 'No output received.' });
      }
    });
  });
}