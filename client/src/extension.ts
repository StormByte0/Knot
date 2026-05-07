/**
 * Knot v2 — Client Extension Entry Point
 *
 * Activates the Knot language server, registers all 13 commands,
 * manages the status bar, and provides comprehensive logging at
 * every step for post-packaging diagnostics.
 *
 * Logging:
 *   - Every activation step is logged to the "Knot" output channel
 *   - Server start/stop/error lifecycle events are captured
 *   - Format detection and changes are logged
 *   - Configuration changes are logged
 *   - All errors include stack traces
 *
 * Debugging:
 *   - Open Output panel → select "Knot" channel
 *   - Run "Knot: Show Status" for a quick health check
 *   - Set `knot.logLevel` to `debug` or `trace` for verbose output
 */

import * as path from 'path';
import * as vscode from 'vscode';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
  NotificationType,
  State,
} from 'vscode-languageclient/node';
import { StatusBar } from './statusBar';
import { LogManager, LogLevel } from './logManager';
import { registerLspCommands } from './commands/lspCommands';
import { registerBuildCommands } from './commands/buildCommands';
import { MenuProvider } from './ui/menuProvider';
import { StatusPanel } from './ui/statusPanel';

let client: LanguageClient | undefined;
let statusBar: StatusBar | undefined;

// Extension version — keep in sync with package.json
const EXTENSION_VERSION = '2.0.0';

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const log = LogManager.instance;

  // ─── Activation Header ──────────────────────────────────────
  log.logActivationHeader(EXTENSION_VERSION);
  const endActivation = log.startTimer('activate');

  // ─── Sync Log Level ─────────────────────────────────────────
  log.syncLevelFromConfig();
  log.info('Extension activating...');

  // ─── Verify Environment ─────────────────────────────────────
  logEnvironmentInfo(context);

  // ─── Server Setup ───────────────────────────────────────────
  const serverModule = context.asAbsolutePath(
    path.join('server', 'out', 'src', 'server.js'),
  );

  log.info(`Server module path: ${serverModule}`);

  // Check if server file actually exists
  try {
    const fs = require('fs');
    const exists = fs.existsSync(serverModule);
    if (!exists) {
      log.error(`Server module NOT FOUND at: ${serverModule}`);
      log.error('The extension will fail to start. Try rebuilding with: npm run build');
      vscode.window.showErrorMessage(
        `Knot: Server module not found. Run "npm run build" and reload.`,
      );
      endActivation();
      return;
    }
    const stats = fs.statSync(serverModule);
    log.info(`Server module found: ${stats.size} bytes, modified ${stats.mtime.toISOString()}`);
  } catch (err) {
    log.warn('Could not verify server module existence', err);
  }

  const debugOptions = { execArgv: ['--nolazy', '--inspect=6009'] };

  const serverOptions: ServerOptions = {
    run: { module: serverModule, transport: TransportKind.ipc },
    debug: {
      module: serverModule,
      transport: TransportKind.ipc,
      options: debugOptions,
    },
  };

  log.info('Server options configured (IPC transport)');

  // ─── Client Options ─────────────────────────────────────────
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
    outputChannel: log.channel,
  };

  log.info('Client options configured', {
    documentSelector: clientOptions.documentSelector,
  });

  // ─── Create Client ──────────────────────────────────────────
  client = new LanguageClient(
    'knot',
    'Knot Language Server',
    serverOptions,
    clientOptions,
  );

  log.info('LanguageClient instance created');

  // ─── Monitor Client State ───────────────────────────────────
  client.onDidChangeState(event => {
    const stateLabels: Record<number, string> = {
      [State.Running]: 'Running',
      [State.Starting]: 'Starting',
      [State.Stopped]: 'Stopped',
    };
    const oldState = stateLabels[event.oldState] ?? `Unknown(${event.oldState})`;
    const newState = stateLabels[event.newState] ?? `Unknown(${event.newState})`;
    log.info(`Client state changed: ${oldState} → ${newState}`);

    if (event.newState === State.Running) {
      statusBar?.updateStatus('running');
    } else if (event.newState === State.Stopped) {
      statusBar?.updateStatus('idle');
      // If the server stopped unexpectedly (not during deactivate), offer to restart
      if (event.oldState === State.Running) {
        log.error('Server stopped unexpectedly — was running, now stopped');
        statusBar?.updateStatus('error');
        const action = vscode.window.showErrorMessage(
          'Knot: Language server stopped unexpectedly. Restart?',
          'Restart',
          'Show Output',
          'Ignore',
        );
        action.then(choice => {
          if (choice === 'Restart' && client) {
            log.info('User chose to restart server after unexpected stop');
            client.start().catch(err => {
              log.error('Failed to restart server after crash', err);
            });
          } else if (choice === 'Show Output') {
            log.show();
          }
        });
      }
    }
  });

  // ─── Start Client ───────────────────────────────────────────
  log.info('Starting language client...');
  const endClientStart = log.startTimer('client.start');

  try {
    await client.start();
    endClientStart();
    log.info('Language client started successfully');
  } catch (err) {
    endClientStart();
    log.error('FAILED to start language client', err);
    statusBar?.updateStatus('error');
    vscode.window.showErrorMessage(
      `Knot: Failed to start language server. Check Output → "Knot" for details. Error: ${err}`,
    );
    endActivation();
    return;
  }

  // ─── Status Bar ─────────────────────────────────────────────
  statusBar = new StatusBar(client);
  context.subscriptions.push(statusBar);
  log.info('Status bar created');

  // ─── Register Commands ──────────────────────────────────────
  const endCommands = log.startTimer('registerCommands');
  registerLspCommands(context, client, statusBar);
  registerBuildCommands(context, client, statusBar);
  endCommands();
  log.info('All commands registered');

  // ─── Menu Provider ──────────────────────────────────────────
  const menuProvider = new MenuProvider(client);
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.mainMenu', () => menuProvider.showMainMenu()),
  );
  log.info('Menu provider registered');

  // ─── Status Panel ───────────────────────────────────────────
  const statusPanel = new StatusPanel(client, statusBar);
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.showStatus', () => statusPanel.show()),
  );
  log.info('Status panel registered');

  // ─── Listen for Server Notifications ─────────────────────────
  client.onNotification(
    new NotificationType<{ formatId: string; formatName: string }>('knot/formatChanged'),
    (params) => {
      log.info(`Format changed: ${params.formatName} (${params.formatId})`);
      statusBar?.updateFormat(params.formatId, params.formatName);
    },
  );

  // Listen for server log messages (forwarded from LSP window/logMessage)
  client.onNotification(
    new NotificationType<{ type: number; message: string }>('window/logMessage'),
    (params) => {
      const levelMap: Record<number, string> = { 1: 'ERROR', 2: 'WARN', 3: 'INFO', 4: 'LOG' };
      const label = levelMap[params.type] ?? 'LOG';
      log.info(`[server] [${label}] ${params.message}`);
    },
  );

  // Listen for server status updates (structured health info)
  client.onNotification(
    new NotificationType<{
      state: string;
      formatId: string;
      formatName: string;
      passageCount: number;
      openDocuments: number;
      message?: string;
    }>('knot/serverStatus'),
    (params) => {
      log.info(
        `[server] Status: ${params.state} | Format: ${params.formatName} (${params.formatId}) | ` +
        `Passages: ${params.passageCount} | Open docs: ${params.openDocuments}` +
        (params.message ? ` | Message: ${params.message}` : ''),
      );

      // Update status bar from server status
      if (params.formatId && params.formatName) {
        statusBar?.updateFormat(params.formatId, params.formatName);
      }
      // Update passage count immediately instead of waiting for 5-second poll
      if (typeof params.passageCount === 'number') {
        statusBar?.updatePassageCount(params.passageCount);
      }
    },
  );

  log.info('Server notification listeners registered');

  // ─── Watch Configuration Changes ────────────────────────────
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(event => {
      if (event.affectsConfiguration('knot')) {
        log.info('Configuration changed — syncing');
        log.syncLevelFromConfig();
        statusBar?.refreshFromConfig();
      }
    }),
  );
  log.info('Configuration watcher registered');

  // ─── Track Open Documents ────────────────────────────────────
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(doc => {
      if (doc.languageId === 'twine' || doc.fileName.endsWith('.tw') || doc.fileName.endsWith('.twee')) {
        log.debug(`Document opened: ${doc.uri.fsPath} (${doc.languageId}, ${doc.lineCount} lines)`);
      }
    }),
  );
  context.subscriptions.push(
    vscode.workspace.onDidCloseTextDocument(doc => {
      if (doc.languageId === 'twine' || doc.fileName.endsWith('.tw') || doc.fileName.endsWith('.twee')) {
        log.debug(`Document closed: ${doc.uri.fsPath}`);
      }
    }),
  );
  log.info('Document watchers registered');

  // ─── Show Activation Info ────────────────────────────────────
  endActivation();
  log.info('Extension activated successfully');

  // Show a non-blocking info message (auto-dismisses)
  const openDocs = vscode.workspace.textDocuments.filter(
    d => d.languageId === 'twine' || d.fileName.endsWith('.tw') || d.fileName.endsWith('.twee'),
  );
  log.info(`Workspace has ${openDocs.length} open Twee documents`);

  vscode.window.showInformationMessage(`Knot v${EXTENSION_VERSION} started`);
}

export async function deactivate(): Promise<void> {
  const log = LogManager.instance;
  log.info('Extension deactivating...');

  if (statusBar) {
    statusBar.dispose();
    statusBar = undefined;
    log.debug('Status bar disposed');
  }

  if (client) {
    log.info('Stopping language client...');
    try {
      await client.stop();
      log.info('Language client stopped');
    } catch (err) {
      log.error('Error stopping language client', err);
    }
    client = undefined;
  }

  log.info('Extension deactivated');
  LogManager.reset();
}

// ─── Helpers ──────────────────────────────────────────────────

function logEnvironmentInfo(context: vscode.ExtensionContext): void {
  const log = LogManager.instance;

  log.info(`VS Code version: ${vscode.version}`);
  log.info(`Extension path: ${context.extensionPath}`);
  log.info(`Storage path: ${context.storagePath ?? '(none)'}`);
  log.info(`Global storage: ${context.globalStoragePath ?? '(none)'}`);

  const workspaceFolders = vscode.workspace.workspaceFolders;
  if (workspaceFolders && workspaceFolders.length > 0) {
    log.info(`Workspace folders: ${workspaceFolders.map(f => f.uri.fsPath).join(', ')}`);
  } else {
    log.warn('No workspace folder open');
  }

  // Check if any .tw/.twee files exist in the workspace
  vscode.workspace.findFiles('**/*.tw').then(twFiles => {
    vscode.workspace.findFiles('**/*.twee').then(tweeFiles => {
      const total = twFiles.length + tweeFiles.length;
      log.info(`Found ${twFiles.length} .tw files and ${tweeFiles.length} .twee files in workspace`);
      if (total === 0) {
        log.warn('No Twee files found — extension may not activate features');
      }
    });
  }, err => {
    log.warn('Could not scan workspace for Twee files', err);
  });
}
