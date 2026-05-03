import * as path from 'node:path';
import * as vscode from 'vscode';
import {
  CloseAction, ErrorAction, LanguageClient,
  LanguageClientOptions, ServerOptions, State, TransportKind,
} from 'vscode-languageclient/node';

import { TweegoIntegration }    from './tweegoIntegration';
import { registerLspCommands }  from './commands/lspCommands';
import { registerBuildCommands } from './commands/buildCommands';
import {
  createStatusBar, setLspState, setLspStarting, setBuildState, setStoryData,
} from './statusBar';

// ---------------------------------------------------------------------------
// Module-level singletons
// ---------------------------------------------------------------------------

let client:  LanguageClient | undefined;
let tweego:  TweegoIntegration | undefined;
let channel: vscode.OutputChannel | undefined;
let extCtx:  vscode.ExtensionContext | undefined;

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

function readProjectConfig() {
  const cfg = vscode.workspace.getConfiguration('knot.project');
  return {
    include: cfg.get<string[]>('include', []),
    exclude: cfg.get<string[]>('exclude', []),
  };
}

function readLintConfig() {
  const cfg = vscode.workspace.getConfiguration('knot.lint');
  return {
    unknownPassage:     cfg.get<string>('unknownPassage', 'warning'),
    unknownMacro:       cfg.get<string>('unknownMacro', 'warning'),
    duplicatePassage:   cfg.get<string>('duplicatePassage', 'error'),
    typeMismatch:       cfg.get<string>('typeMismatch', 'error'),
    unreachablePassage: cfg.get<string>('unreachablePassage', 'warning'),
    containerStructure: cfg.get<string>('containerStructure', 'error'),
  };
}

// ---------------------------------------------------------------------------
// Activate
// ---------------------------------------------------------------------------

export function activate(context: vscode.ExtensionContext): void {
  extCtx  = context;
  channel = vscode.window.createOutputChannel('knot Language Server');
  context.subscriptions.push(channel);

  vscode.commands.executeCommand('setContext', 'knot.building', false);
  vscode.commands.executeCommand('setContext', 'knot.watching', false);

  createStatusBar(context);

  tweego = new TweegoIntegration(channel);
  context.subscriptions.push({ dispose: () => tweego?.dispose() });

  context.subscriptions.push(tweego.onBuildStateChange.event(s => {
    setBuildState(s);
    vscode.commands.executeCommand('setContext', 'knot.building', s === 'building');
    vscode.commands.executeCommand('setContext', 'knot.watching', s === 'watching');
  }));

  registerLspCommands(context, channel, () => client, restartClient);
  registerBuildCommands(context, () => tweego);

  context.subscriptions.push({ dispose: () => void stopClient() });

  startClient();
  registerConfigWatcher();
  context.subscriptions.push({ dispose: () => configChangeDisposable?.dispose() });
}

// ---------------------------------------------------------------------------
// LSP client lifecycle
// ---------------------------------------------------------------------------

function startClient(): void {
  if (!extCtx || !channel) return;

  const serverModule = extCtx.asAbsolutePath(
    path.join('server', 'out', 'src', 'lspServer.js'),
  );

  const serverOptions: ServerOptions = {
    run:   { module: serverModule, transport: TransportKind.ipc },
    debug: {
      module: serverModule, transport: TransportKind.ipc,
      options: { execArgv: ['--nolazy', '--inspect=6009'] },
    },
  };

  const fileWatcher = vscode.workspace.createFileSystemWatcher('**/*.{tw,twee}');

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { language: 'twine' },
      { pattern:  '**/*.{tw,twee}' },
    ],
    outputChannel: channel,
    synchronize: { fileEvents: fileWatcher },
    initializationOptions: {
      exclude: readProjectConfig().exclude,
      lint: readLintConfig(),
    },
    initializationFailedHandler: err => {
      channel?.appendLine(`[init-failed] ${String(err)}`);
      setLspState('error');
      return false;
    },
    errorHandler: {
      error: err => ({ action: ErrorAction.Continue, message: err.message }),
      closed: ()  => { setLspState('error'); return { action: CloseAction.DoNotRestart }; },
    },
  };

  client = new LanguageClient(
    'knot', 'knot Language Server',
    serverOptions, clientOptions,
  );

  setLspStarting();

  client.onDidChangeState(ev => {
    if (ev.newState === State.Starting) setLspStarting();
    if (ev.newState === State.Stopped)  setLspState('error');
  });

  client.start().then(() => {
    if (!client) return;

    client.onNotification('knot/serverReady', () => {
      setLspState('ready');
      channel?.appendLine('[status] server ready');
    });

    client.onNotification('knot/progressStart', () => setLspStarting());

    client.onNotification('knot/storyDataUpdated', (data: {
      ifid: string | null;
      format: string | null;
      formatVersion: string | null;
      start: string | null;
      passageCount: number;
    }) => { setStoryData(data); });

  }).catch(err => {
    setLspState('error');
    channel?.appendLine(`[startup-error] ${String(err)}`);
  });

  void populateWorkspace(client);
}

async function populateWorkspace(lspClient: LanguageClient): Promise<void> {
  const statePromise = new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('timeout')), 30_000);
    const disposable = lspClient.onDidChangeState(ev => {
      if (ev.newState === State.Running) { clearTimeout(timeout); disposable.dispose(); resolve(); }
      if (ev.newState === State.Stopped) { clearTimeout(timeout); disposable.dispose(); reject(new Error('stopped')); }
    });
  });
  try {
    await statePromise;
  } catch {
    return;
  }

  const proj = readProjectConfig();

  const includeGlob = proj.include.length > 0
    ? `{${proj.include.map(p => `**/${p}/**/*.{tw,twee}`).join(',')}}`
    : '**/*.{tw,twee}';

  const excludeGlob = proj.exclude.length > 0
    ? `{${proj.exclude.join(',')}}`
    : undefined;

  let files: vscode.Uri[];
  try {
    files = await vscode.workspace.findFiles(includeGlob, excludeGlob);
  } catch (err) {
    channel?.appendLine(`[populate] findFiles failed: ${err}`);
    return;
  }

  channel?.appendLine(`[populate] Found ${files.length} .tw/.twee file(s)`);

  const BATCH = 20;
  for (let i = 0; i < files.length; i += BATCH) {
    const batch = files.slice(i, i + BATCH);
    await Promise.allSettled(
      batch.map(uri =>
        Promise.resolve(vscode.workspace.openTextDocument(uri)).catch(err => {
          channel?.appendLine(`[populate] Could not open ${uri.fsPath}: ${err}`);
        }),
      ),
    );
  }

  channel?.appendLine('[populate] Workspace population complete');
}

// ---------------------------------------------------------------------------
// Config change handler
// ---------------------------------------------------------------------------

let configChangeDisposable: vscode.Disposable | undefined;

function registerConfigWatcher(): void {
  configChangeDisposable?.dispose();
  configChangeDisposable = vscode.workspace.onDidChangeConfiguration(async e => {
    if (
      e.affectsConfiguration('knot.project.include') ||
      e.affectsConfiguration('knot.project.exclude') ||
      e.affectsConfiguration('knot.lint')
    ) {
      const choice = await vscode.window.showInformationMessage(
        'knot Language Server: include/exclude settings changed. Restart to apply?',
        'Restart', 'Later',
      );
      if (choice === 'Restart') await restartClient();
    }
  });
}

// ---------------------------------------------------------------------------
// Restart / stop
// ---------------------------------------------------------------------------

async function restartClient(): Promise<void> {
  setLspStarting();
  await stopClient();
  startClient();
}

async function stopClient(): Promise<void> {
  const c = client;
  client = undefined;
  if (!c) return;
  try { await c.stop(2000); } catch { /* ignore */ }
}

export async function deactivate(): Promise<void> {
  configChangeDisposable?.dispose();
  tweego?.dispose();
  await stopClient();
}