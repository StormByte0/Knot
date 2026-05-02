import * as cp from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface TweegoConfig {
  execPath:             string;
  outputFile:           string;
  storyFormatsDirectory: string;
  formatOverride:       string;
  modulePaths:          string[];
  headFile:             string;
  noTrim:               boolean;
  logFiles:             boolean;
  extraArgs:            string;
}

export interface BuildResult {
  success:    boolean;
  errors:     TweegoError[];
  stdout:     string;
  stderr:     string;
  durationMs: number;
}

export interface TweegoError {
  file:    string | null;
  line:    number | null;
  message: string;
  uri:     vscode.Uri | null;
}

export interface FormatEntry {
  id:      string;
  name:    string;
  version: string;
}

export type BuildState = 'idle' | 'building' | 'success' | 'failed' | 'watching';

// ---------------------------------------------------------------------------
// Config reader
// ---------------------------------------------------------------------------

export function readTweegoConfig(): TweegoConfig {
  const tweego = vscode.workspace.getConfiguration('knot.tweego');
  const project = vscode.workspace.getConfiguration('knot.project');
  return {
    execPath:              tweego.get<string>('path',               'tweego'),
    outputFile:            tweego.get<string>('outputFile',         'build/index.html'),
    storyFormatsDirectory: project.get<string>('storyFormatsDirectory', '.storyformats'),
    formatOverride:        tweego.get<string>('formatOverride',     ''),
    modulePaths:           tweego.get<string[]>('modulePaths',       []),
    headFile:              tweego.get<string>('headFile',            ''),
    noTrim:                tweego.get<boolean>('noTrim',             false),
    logFiles:              tweego.get<boolean>('logFiles',           false),
    extraArgs:             tweego.get<string>('extraArgs',           ''),
  };
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

function resolveWorkspacePath(p: string, workspaceRoot: string): string {
  if (!p) return p;
  return path.isAbsolute(p) ? p : path.join(workspaceRoot, p);
}

// ---------------------------------------------------------------------------
// CLI argument builder
// ---------------------------------------------------------------------------

function buildArgs(config: TweegoConfig, workspaceRoot: string, storyFilesDir: string, testMode: boolean): string[] {
  const args: string[] = ['-o', resolveWorkspacePath(config.outputFile, workspaceRoot)];
  if (config.formatOverride) args.push('-f', config.formatOverride);
  for (const mp of config.modulePaths) args.push('-m', resolveWorkspacePath(mp, workspaceRoot));
  if (config.headFile) args.push('--head', resolveWorkspacePath(config.headFile, workspaceRoot));
  if (config.noTrim)   args.push('--no-trim');
  if (config.logFiles) args.push('--log-files');
  if (testMode)        args.push('-t');
  if (config.extraArgs.trim()) args.push(...config.extraArgs.trim().split(/\s+/));
  // Point tweego at the story files directory, not the entire workspace root
  args.push(resolveWorkspacePath(storyFilesDir || '.', workspaceRoot));
  return args;
}

// ---------------------------------------------------------------------------
// Environment builder
// ---------------------------------------------------------------------------

function buildEnv(config: TweegoConfig, workspaceRoot: string): NodeJS.ProcessEnv {
  const env = { ...process.env };
  const sep = process.platform === 'win32' ? ';' : ':';
  const fmtDir = resolveWorkspacePath(config.storyFormatsDirectory, workspaceRoot);
  // Always append the project's storyformats dir to TWEEGO_PATH
  env['TWEEGO_PATH'] = env['TWEEGO_PATH'] ? `${env['TWEEGO_PATH']}${sep}${fmtDir}` : fmtDir;
  return env;
}

// ---------------------------------------------------------------------------
// Output file preparation
// ---------------------------------------------------------------------------

function prepareOutputFile(outputFile: string, workspaceRoot: string, channel: vscode.OutputChannel): string | null {
  const resolved = resolveWorkspacePath(outputFile, workspaceRoot);
  try {
    fs.mkdirSync(path.dirname(resolved), { recursive: true });
  } catch (err) {
    channel.appendLine(`[build] Warning: could not create output directory: ${(err as Error).message}`);
  }
  if (fs.existsSync(resolved)) {
    try {
      fs.unlinkSync(resolved);
    } catch (err) {
      const msg = `Output file "${resolved}" is locked — ${(err as Error).message}. Close any process holding it.`;
      channel.appendLine(`[build] Warning: ${msg}`);
      return msg;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Stderr parser
// ---------------------------------------------------------------------------

const LINE_ERROR_RE = /^(?:error|fatal|warning|tweego):\s*(.+)$/i;
const FILE_LINE_RE  = /^(.+?):(\d+):\s*(.+)$/;
const SKIP_PREFIXES = new Set(['error', 'warning', 'fatal', 'tweego']);

function parseStderr(stderr: string, workspaceRoot: string): TweegoError[] {
  const errors: TweegoError[] = [];
  for (const rawLine of stderr.split('\n')) {
    const line = rawLine.trim();
    if (!line) continue;
    const flMatch = line.match(FILE_LINE_RE);
    if (flMatch) {
      const [, file, lineStr, message] = flMatch;
      if (!SKIP_PREFIXES.has(file!.toLowerCase())) {
        const absFile = path.isAbsolute(file!) ? file! : path.join(workspaceRoot, file!);
        errors.push({ file: absFile, line: parseInt(lineStr!, 10), message: message!, uri: vscode.Uri.file(absFile) });
        continue;
      }
    }
    const errMatch = line.match(LINE_ERROR_RE);
    if (errMatch) errors.push({ file: null, line: null, message: errMatch[1]!, uri: null });
  }
  return errors;
}

// ---------------------------------------------------------------------------
// Binary verification
// ---------------------------------------------------------------------------

function runVerify(execPath: string): Promise<{ ok: boolean; version: string }> {
  return new Promise((resolve) => {
    cp.execFile(execPath, ['-v'], { timeout: 8_000 }, (_err, stdout, stderr) => {
      const out   = (stdout || stderr || '').trim();
      const match = out.match(/(\d+\.\d+(?:\.\d+)?)/);
      resolve({ ok: Boolean(match), version: match ? match[1]! : out });
    });
  });
}

// ---------------------------------------------------------------------------
// TweegoIntegration
// ---------------------------------------------------------------------------

export class TweegoIntegration {
  private watchProcess:         cp.ChildProcess | null = null;
  private diagnosticCollection: vscode.DiagnosticCollection;
  private outputChannel:        vscode.OutputChannel;

  readonly onBuildStateChange = new vscode.EventEmitter<BuildState>();
  private _buildState: BuildState = 'idle';

  constructor(outputChannel: vscode.OutputChannel) {
    this.outputChannel        = outputChannel;
    this.diagnosticCollection = vscode.languages.createDiagnosticCollection('knot-tweego');
  }

  get buildState(): BuildState { return this._buildState; }
  get isWatching(): boolean    { return this.watchProcess !== null; }

  private setState(s: BuildState): void {
    this._buildState = s;
    this.onBuildStateChange.fire(s);
  }

  private getWorkspaceRoot(): string | null {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
  }

  private getStoryFilesDir(): string {
    return vscode.workspace.getConfiguration('knot.project')
      .get<string>('storyFilesDirectory', 'src');
  }

  // ── Build ─────────────────────────────────────────────────────────────────

  async build(testMode = false): Promise<BuildResult> {
    if (this._buildState === 'building') {
      vscode.window.showWarningMessage('knot: A build is already in progress.');
      return { success: false, errors: [], stdout: '', stderr: '', durationMs: 0 };
    }
    const workspaceRoot = this.getWorkspaceRoot();
    if (!workspaceRoot) {
      vscode.window.showErrorMessage('knot: No workspace folder open.');
      return { success: false, errors: [], stdout: '', stderr: '', durationMs: 0 };
    }
    const config = readTweegoConfig();
    const prepWarning = prepareOutputFile(config.outputFile, workspaceRoot, this.outputChannel);
    if (prepWarning) {
      vscode.window.showWarningMessage(`knot: ${prepWarning}`, 'Show Output')
        .then(p => { if (p === 'Show Output') this.outputChannel.show(); });
    }

    const result = await this.runTweego(config, workspaceRoot, testMode);
    this.publishDiagnostics(result.errors);

    if (result.success) {
      this.setState('success');
      this.outputChannel.appendLine(`\n[build] Done in ${result.durationMs}ms → ${config.outputFile}`);
    } else {
      this.setState('failed');
      this.outputChannel.appendLine(`\n[build] Failed in ${result.durationMs}ms`);
      if (result.errors.length === 0 && result.stderr.trim()) {
        this.outputChannel.appendLine('[build] Raw stderr:');
        this.outputChannel.appendLine(result.stderr.trim());
        this.publishDiagnostics([{ file: null, line: null, message: 'Build failed — see knot output channel', uri: null }]);
      } else {
        for (const err of result.errors) {
          const loc = err.file ? `${path.basename(err.file)}:${err.line ?? '?'} ` : '';
          this.outputChannel.appendLine(`  ${loc}${err.message}`);
        }
      }
      vscode.window.showErrorMessage(
        `knot: Build failed — ${result.errors.length > 0 ? result.errors[0]!.message : 'see output for details'}`,
        'Show Output',
      ).then(p => { if (p === 'Show Output') this.outputChannel.show(); });
    }
    return result;
  }

  // ── Watch ─────────────────────────────────────────────────────────────────

  async startWatch(): Promise<void> {
    if (this.watchProcess) { vscode.window.showWarningMessage('knot: Watch mode already running.'); return; }
    const workspaceRoot = this.getWorkspaceRoot();
    if (!workspaceRoot) { vscode.window.showErrorMessage('knot: No workspace folder open.'); return; }
    const config = readTweegoConfig();
    prepareOutputFile(config.outputFile, workspaceRoot, this.outputChannel);
    const storyFilesDir = this.getStoryFilesDir();
    const args = [...buildArgs(config, workspaceRoot, storyFilesDir, false), '-w'];
    const env  = buildEnv(config, workspaceRoot);
    this.outputChannel.appendLine(`\n[watch] ${config.execPath} ${args.join(' ')}`);
    this.setState('watching');
    const proc = cp.spawn(config.execPath, args, { cwd: workspaceRoot, env });
    this.watchProcess = proc;
    proc.stdout?.on('data', (chunk: Buffer) => {
      const text = chunk.toString();
      this.outputChannel.append(text);
      for (const line of text.split('\n')) {
        if (line.startsWith('BUILDING:')) { this.diagnosticCollection.clear(); this.setState('building'); }
      }
    });
    proc.stderr?.on('data', (chunk: Buffer) => {
      const text   = chunk.toString();
      this.outputChannel.append(text);
      const errors = parseStderr(text, workspaceRoot);
      if (errors.length > 0) {
        this.publishDiagnostics(errors);
        this.setState('failed');
        vscode.window.showErrorMessage(`knot: Watch build failed — ${errors[0]!.message}`, 'Show Output')
          .then(p => { if (p === 'Show Output') this.outputChannel.show(); });
      } else if (this._buildState === 'building') {
        this.diagnosticCollection.clear();
        this.setState('watching');
      }
    });
    proc.on('close', (code) => {
      this.outputChannel.appendLine(`\n[watch] Process exited (code ${code ?? 'unknown'})`);
      this.watchProcess = null;
      this.setState('idle');
    });
    proc.on('error', (err) => {
      this.outputChannel.appendLine(`\n[watch] Spawn error: ${(err as Error).message}`);
      this.watchProcess = null;
      this.setState('failed');
      this.handleExecError(err as NodeJS.ErrnoException, config.execPath);
    });
  }

  stopWatch(): void {
    if (!this.watchProcess) return;
    this.watchProcess.kill();
    this.watchProcess = null;
    this.setState('idle');
    this.outputChannel.appendLine('\n[watch] Stopped.');
  }

  // ── Format listing ─────────────────────────────────────────────────────────

  async listFormats(): Promise<FormatEntry[]> {
    const config        = readTweegoConfig();
    const workspaceRoot = this.getWorkspaceRoot() ?? process.cwd();
    const env           = buildEnv(config, workspaceRoot);
    return new Promise((resolve) => {
      cp.execFile(config.execPath, ['--list-formats'], { cwd: workspaceRoot, env, timeout: 10_000 },
        (_err, stdout, stderr) => {
          const out = (stdout || stderr || '').trim();
          const formats: FormatEntry[] = [];
          for (const line of out.split('\n')) {
            const m = line.match(/^\s*([\w-]+)\s+\(([^,]+),\s*v([^)]+)\)/);
            if (m) formats.push({ id: m[1]!, name: m[2]!.trim(), version: m[3]!.trim() });
          }
          this.outputChannel.appendLine('\n[formats]\n' + out);
          resolve(formats);
        });
    });
  }

  /** Verify using the currently saved config path. */
  async verifyBinary(): Promise<{ ok: boolean; version: string }> {
    return runVerify(readTweegoConfig().execPath);
  }

  /**
   * Verify a specific path — called by the settings panel so verification
   * uses what's in the input field before the debounced save fires.
   */
  async verifyBinaryAt(execPath: string): Promise<{ ok: boolean; version: string }> {
    return runVerify(execPath.trim() || 'tweego');
  }

  dispose(): void {
    this.stopWatch();
    this.diagnosticCollection.dispose();
    this.onBuildStateChange.dispose();
  }

  // ── Private ───────────────────────────────────────────────────────────────

  private async runTweego(config: TweegoConfig, workspaceRoot: string, testMode: boolean): Promise<BuildResult> {
    const storyFilesDir = this.getStoryFilesDir();
    const args  = buildArgs(config, workspaceRoot, storyFilesDir, testMode);
    const env   = buildEnv(config, workspaceRoot);
    const start = Date.now();
    this.setState('building');
    this.diagnosticCollection.clear();
    this.outputChannel.appendLine(`\n[build] ${config.execPath} ${args.join(' ')}`);
    return new Promise((resolve) => {
      let stdout = '', stderr = '';
      const proc = cp.spawn(config.execPath, args, { cwd: workspaceRoot, env });
      proc.stdout?.on('data', (d: Buffer) => { const t = d.toString(); stdout += t; this.outputChannel.append(t); });
      proc.stderr?.on('data', (d: Buffer) => { const t = d.toString(); stderr += t; this.outputChannel.append(t); });
      proc.on('error', (err) => {
        this.handleExecError(err as NodeJS.ErrnoException, config.execPath);
        resolve({ success: false, errors: [{ file: null, line: null, message: (err as Error).message, uri: null }], stdout, stderr, durationMs: Date.now() - start });
      });
      proc.on('close', (code) => {
        resolve({ success: code === 0, errors: parseStderr(stderr, workspaceRoot), stdout, stderr, durationMs: Date.now() - start });
      });
    });
  }

  private publishDiagnostics(errors: TweegoError[]): void {
    this.diagnosticCollection.clear();
    const byUri = new Map<string, vscode.Diagnostic[]>();
    for (const err of errors) {
      if (!err.uri) continue;
      const key   = err.uri.toString();
      const diags = byUri.get(key) ?? [];
      const line  = err.line != null ? Math.max(0, err.line - 1) : 0;
      const diag  = new vscode.Diagnostic(new vscode.Range(line, 0, line, 999), err.message, vscode.DiagnosticSeverity.Error);
      diag.source = 'tweego';
      diags.push(diag);
      byUri.set(key, diags);
    }
    for (const [uriStr, diags] of byUri) {
      this.diagnosticCollection.set(vscode.Uri.parse(uriStr), diags);
    }
  }

  private handleExecError(err: NodeJS.ErrnoException, execPath: string): void {
    if (err.code === 'ENOENT') {
      vscode.window.showErrorMessage(
        `knot: Tweego not found at "${execPath}". Configure the path in knot Settings.`, 'Open Settings',
      ).then(p => { if (p === 'Open Settings') vscode.commands.executeCommand('knot.openSettings'); });
    } else {
      vscode.window.showErrorMessage(`knot: Tweego spawn error — ${err.message}`);
    }
  }
}