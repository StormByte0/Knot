/**
 * Knot v2 — Client Extension Entry Point
 *
 * Activates the Knot language server, registers all 13 commands
 * declared in package.json, and manages the status bar.
 *
 * Promises fulfilled (from package.json):
 *   - Extension activation on .tw/.twee files
 *   - Language client lifecycle
 *   - All 13 command registrations
 *   - Status bar (server status + active format)
 *   - Configuration change forwarding
 */

import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
  NotificationType,
} from 'vscode-languageclient/node';
import { StatusBar } from './statusBar';
import { registerLspCommands } from './commands/lspCommands';
import { registerBuildCommands } from './commands/buildCommands';
import { MenuProvider } from './ui/menuProvider';

let client: LanguageClient | undefined;
let statusBar: StatusBar | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  // ─── Server Setup ──────────────────────────────────────────
  const serverModule = context.asAbsolutePath(
    path.join('server', 'out', 'src', 'server.js'),
  );

  const debugOptions = { execArgv: ['--nolazy', '--inspect=6009'] };

  const serverOptions: ServerOptions = {
    run: { module: serverModule, transport: TransportKind.ipc },
    debug: {
      module: serverModule,
      transport: TransportKind.ipc,
      options: debugOptions,
    },
  };

  // ─── Client Options ────────────────────────────────────────
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'twine' },
      { scheme: 'file', pattern: '**/*.tw' },
      { scheme: 'file', pattern: '**/*.twee' },
    ],
    synchronize: {
      configurationSection: 'knot',
      fileEvents: [
        vscode.workspace.createFileSystemWatcher('**/*.tw'),
        vscode.workspace.createFileSystemWatcher('**/*.twee'),
      ],
    },
  };

  // ─── Create & Start Client ─────────────────────────────────
  client = new LanguageClient(
    'knot',
    'Knot Language Server',
    serverOptions,
    clientOptions,
  );

  // Start the client — this also starts the server
  await client.start();

  // ─── Status Bar ────────────────────────────────────────────
  statusBar = new StatusBar(client);
  context.subscriptions.push(statusBar);

  // ─── Register Commands ─────────────────────────────────────
  registerLspCommands(context, client, statusBar);
  registerBuildCommands(context, client, statusBar);

  // ─── Menu Provider ─────────────────────────────────────────
  const menuProvider = new MenuProvider(client);
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.mainMenu', () => menuProvider.showMainMenu()),
  );

  // ─── Listen for server format change notifications ──────────
  client.onNotification(
    new NotificationType<{ formatId: string; formatName: string }>('knot/formatChanged'),
    (params) => {
      statusBar?.updateFormat(params.formatId, params.formatName);
    },
  );

  // ─── Watch Configuration Changes ───────────────────────────
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(event => {
      if (event.affectsConfiguration('knot')) {
        statusBar?.refreshFromConfig();
      }
    }),
  );

  vscode.window.showInformationMessage('Knot v2 language server started');
}

export async function deactivate(): Promise<void> {
  if (statusBar) {
    statusBar.dispose();
    statusBar = undefined;
  }
  if (client) {
    await client.stop();
    client = undefined;
  }
}