/**
 * Knot v2 — Build Commands
 *
 * Implements all build-related commands declared in package.json:
 *   knot.build
 *   knot.buildTest
 *   knot.verifyTweego
 *   knot.startWatch
 *   knot.stopWatch
 *
 * These handle Tweego build integration, independent of story format.
 */

import * as vscode from 'vscode';
import * as cp from 'child_process';
import { LanguageClient } from 'vscode-languageclient/node';
import { StatusBar } from '../statusBar';

export function registerBuildCommands(
  context: vscode.ExtensionContext,
  client: LanguageClient,
  statusBar: StatusBar,
): void {
  let watchProcess: cp.ChildProcess | undefined;
  let outputChannel: vscode.OutputChannel | undefined;

  function getOutputChannel(): vscode.OutputChannel {
    if (!outputChannel) {
      outputChannel = vscode.window.createOutputChannel('Knot Build');
      context.subscriptions.push(outputChannel);
    }
    return outputChannel;
  }

  function getTweegoConfig(): {
    path: string;
    outputFile: string;
    formatOverride: string;
    modulePaths: string[];
    headFile: string;
    noTrim: boolean;
    logFiles: boolean;
    extraArgs: string;
    storyFilesDirectory: string;
  } {
    const config = vscode.workspace.getConfiguration('knot');
    return {
      path: config.get<string>('tweego.path', 'tweego'),
      outputFile: config.get<string>('tweego.outputFile', 'dist/index.html'),
      formatOverride: config.get<string>('tweego.formatOverride', ''),
      modulePaths: config.get<string[]>('tweego.modulePaths', []),
      headFile: config.get<string>('tweego.headFile', ''),
      noTrim: config.get<boolean>('tweego.noTrim', false),
      logFiles: config.get<boolean>('tweego.logFiles', false),
      extraArgs: config.get<string>('tweego.extraArgs', ''),
      storyFilesDirectory: config.get<string>('project.storyFilesDirectory', 'src'),
    };
  }

  function buildTweegoArgs(testMode: boolean = false): string[] {
    const cfg = getTweegoConfig();
    const args: string[] = [];

    // Output file
    args.push('-o', cfg.outputFile);

    // Format override
    if (cfg.formatOverride) {
      args.push('--format', cfg.formatOverride);
    }

    // Module paths
    for (const modPath of cfg.modulePaths) {
      args.push('-m', modPath);
    }

    // Head file
    if (cfg.headFile) {
      args.push('--head', cfg.headFile);
    }

    // No trim
    if (cfg.noTrim) {
      args.push('--no-trim');
    }

    // Log files
    if (cfg.logFiles) {
      args.push('--log-files');
    }

    // Test mode
    if (testMode) {
      args.push('--test');
    }

    // Extra args
    if (cfg.extraArgs) {
      args.push(...cfg.extraArgs.split(/\s+/).filter(Boolean));
    }

    // Source directory
    args.push(cfg.storyFilesDirectory);

    return args;
  }

  // ─── knot.build ────────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.build', async () => {
      const cfg = getTweegoConfig();
      const args = buildTweegoArgs(false);
      const ch = getOutputChannel();

      ch.clear();
      ch.show(true);
      ch.appendLine(`Running: ${cfg.path} ${args.join(' ')}`);
      ch.appendLine('');

      try {
        const result = await execAsync(cfg.path, args, {
          cwd: vscode.workspace.rootPath,
        });
        ch.appendLine(result.stdout);
        if (result.stderr) {
          ch.appendLine(result.stderr);
        }
        ch.appendLine(`\nBuild succeeded: ${cfg.outputFile}`);
        vscode.window.showInformationMessage('Knot: Build succeeded');
      } catch (err: any) {
        ch.appendLine(err.stdout ?? '');
        ch.appendLine(err.stderr ?? '');
        ch.appendLine(`\nBuild failed (exit code ${err.code ?? 'unknown'})`);
        vscode.window.showErrorMessage(`Knot: Build failed — see output for details`);
      }
    }),
  );

  // ─── knot.buildTest ────────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.buildTest', async () => {
      const cfg = getTweegoConfig();
      const args = buildTweegoArgs(true);
      const ch = getOutputChannel();

      ch.clear();
      ch.show(true);
      ch.appendLine(`Running (test mode): ${cfg.path} ${args.join(' ')}`);
      ch.appendLine('');

      try {
        const result = await execAsync(cfg.path, args, {
          cwd: vscode.workspace.rootPath,
        });
        ch.appendLine(result.stdout);
        if (result.stderr) {
          ch.appendLine(result.stderr);
        }
        ch.appendLine('\nTest build succeeded');
        vscode.window.showInformationMessage('Knot: Test build succeeded');
      } catch (err: any) {
        ch.appendLine(err.stdout ?? '');
        ch.appendLine(err.stderr ?? '');
        ch.appendLine(`\nTest build failed (exit code ${err.code ?? 'unknown'})`);
        vscode.window.showErrorMessage('Knot: Test build failed — see output for details');
      }
    }),
  );

  // ─── knot.verifyTweego ─────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.verifyTweego', async () => {
      const cfg = getTweegoConfig();
      const ch = getOutputChannel();

      try {
        const result = await execAsync(cfg.path, ['--version'], {
          cwd: vscode.workspace.rootPath,
        });
        const version = (result.stdout ?? result.stderr ?? '').trim();
        ch.appendLine(`Tweego found: ${version}`);
        vscode.window.showInformationMessage(`Knot: Tweego ${version}`);
      } catch (err) {
        vscode.window.showErrorMessage(
          `Knot: Tweego not found at "${cfg.path}". Make sure it's installed and on your PATH, or set the path in settings.`,
        );
      }
    }),
  );

  // ─── knot.startWatch ──────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.startWatch', async () => {
      if (watchProcess) {
        vscode.window.showWarningMessage('Knot: Watch mode is already running');
        return;
      }

      const cfg = getTweegoConfig();
      const args = [...buildTweegoArgs(false), '--watch'];
      const ch = getOutputChannel();

      ch.clear();
      ch.show(true);
      ch.appendLine(`Watching: ${cfg.path} ${args.join(' ')}`);

      watchProcess = cp.spawn(cfg.path, args, {
        cwd: vscode.workspace.rootPath,
      });

      watchProcess.stdout?.on('data', (data: Buffer) => {
        ch.append(data.toString());
      });

      watchProcess.stderr?.on('data', (data: Buffer) => {
        ch.append(data.toString());
      });

      watchProcess.on('close', (code) => {
        ch.appendLine(`Watch process exited (code ${code})`);
        watchProcess = undefined;
      });

      vscode.window.showInformationMessage('Knot: Watch mode started');
    }),
  );

  // ─── knot.stopWatch ──────────────────────────────────────
  context.subscriptions.push(
    vscode.commands.registerCommand('knot.stopWatch', () => {
      if (!watchProcess) {
        vscode.window.showWarningMessage('Knot: Watch mode is not running');
        return;
      }

      watchProcess.kill();
      watchProcess = undefined;
      vscode.window.showInformationMessage('Knot: Watch mode stopped');
    }),
  );
}

// ─── Utility ─────────────────────────────────────────────────

function execAsync(
  command: string,
  args: string[],
  options?: cp.ExecFileOptions,
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    cp.execFile(command, args, options, (error, stdout, stderr) => {
      if (error) {
        reject(Object.assign(error, { stdout, stderr }));
      } else {
        resolve({ stdout: String(stdout ?? ''), stderr: String(stderr ?? '') });
      }
    });
  });
}
