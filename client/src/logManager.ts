/**
 * Knot v2 — Log Manager
 *
 * Centralized logging for the Knot extension client.
 * Provides structured, leveled logging to the Knot output channel
 * so every step of extension activation, LSP communication, and
 * error handling is visible without a debugger.
 *
 * Usage:
 *   const log = LogManager.instance;
 *   log.info('Extension activating...');
 *   log.error('Failed to start server', err);
 *   log.debug('Handler registered', { handler: 'hover' });
 *
 * Log levels can be controlled via `knot.logLevel` setting.
 */

import * as vscode from 'vscode';

export enum LogLevel {
  Off = 0,
  Error = 1,
  Warn = 2,
  Info = 3,
  Debug = 4,
  Trace = 5,
}

const LEVEL_LABELS: Record<LogLevel, string> = {
  [LogLevel.Off]: 'OFF',
  [LogLevel.Error]: 'ERR ',
  [LogLevel.Warn]: 'WARN',
  [LogLevel.Info]: 'INFO',
  [LogLevel.Debug]: 'DBG ',
  [LogLevel.Trace]: 'TRC ',
};

export class LogManager implements vscode.Disposable {
  private static _instance: LogManager | undefined;
  private outputChannel: vscode.OutputChannel;
  private level: LogLevel = LogLevel.Info;
  private _startTime: number;

  private constructor() {
    this.outputChannel = vscode.window.createOutputChannel('Knot');
    this._startTime = Date.now();
  }

  static get instance(): LogManager {
    if (!LogManager._instance) {
      LogManager._instance = new LogManager();
    }
    return LogManager._instance;
  }

  static reset(): void {
    if (LogManager._instance) {
      LogManager._instance.dispose();
      LogManager._instance = undefined;
    }
  }

  /** Update the log level from configuration. */
  setLevel(level: LogLevel): void {
    this.level = level;
    this.info(`Log level set to ${LEVEL_LABELS[level]}`);
  }

  /** Read log level from VS Code configuration. */
  syncLevelFromConfig(): void {
    const config = vscode.workspace.getConfiguration('knot');
    const levelStr = config.get<string>('logLevel', 'info');
    const level = this.parseLevel(levelStr);
    this.setLevel(level);
  }

  /** Show the output channel. */
  show(): void {
    this.outputChannel.show(true);
  }

  /** Get the underlying output channel (for LanguageClient). */
  get channel(): vscode.OutputChannel {
    return this.outputChannel;
  }

  // ─── Log Methods ─────────────────────────────────────────────

  error(message: string, ...args: unknown[]): void {
    this.log(LogLevel.Error, message, ...args);
  }

  warn(message: string, ...args: unknown[]): void {
    this.log(LogLevel.Warn, message, ...args);
  }

  info(message: string, ...args: unknown[]): void {
    this.log(LogLevel.Info, message, ...args);
  }

  debug(message: string, ...args: unknown[]): void {
    this.log(LogLevel.Debug, message, ...args);
  }

  trace(message: string, ...args: unknown[]): void {
    this.log(LogLevel.Trace, message, ...args);
  }

  // ─── Lifecycle Helpers ────────────────────────────────────────

  /** Log the start of a timed section. Returns an end function. */
  startTimer(label: string): () => void {
    const start = Date.now();
    this.debug(`[${label}] started`);
    return () => {
      const elapsed = Date.now() - start;
      this.debug(`[${label}] completed in ${elapsed}ms`);
    };
  }

  /** Log extension activation header. Clears old output first. */
  logActivationHeader(version: string): void {
    // Clear any stale output from previous sessions
    this.outputChannel.clear();
    this.outputChannel.appendLine('╔══════════════════════════════════════════════════╗');
    this.outputChannel.appendLine('║         Knot — Twine Language Support           ║');
    this.outputChannel.appendLine('╠══════════════════════════════════════════════════╣');
    this.outputChannel.appendLine(`║  Version:     ${version.padEnd(35)}║`);
    this.outputChannel.appendLine(`║  VS Code:     ${vscode.version.padEnd(35)}║`);
    this.outputChannel.appendLine(`║  Session:     ${new Date().toISOString().padEnd(35)}║`);
    this.outputChannel.appendLine('╚══════════════════════════════════════════════════╝');
    this.outputChannel.appendLine('');
  }

  dispose(): void {
    this.outputChannel.dispose();
  }

  // ─── Private ──────────────────────────────────────────────────

  private log(level: LogLevel, message: string, ...args: unknown[]): void {
    if (level > this.level || level === LogLevel.Off) return;

    const timestamp = new Date().toISOString().slice(11, 23); // HH:MM:SS.mmm
    const label = LEVEL_LABELS[level];
    let line = `[${timestamp}] [${label}] ${message}`;

    if (args.length > 0) {
      const extras = args.map(a => {
        if (a instanceof Error) {
          return `\n  Error: ${a.message}\n  Stack: ${a.stack ?? '(no stack)'}`;
        }
        if (typeof a === 'object' && a !== null) {
          try {
            return `\n  ${JSON.stringify(a, null, 2).split('\n').join('\n  ')}`;
          } catch {
            return `\n  [object]`;
          }
        }
        return ` ${String(a)}`;
      }).join('');
      line += extras;
    }

    this.outputChannel.appendLine(line);

    // Also surface errors as status bar updates
    if (level === LogLevel.Error) {
      // Don't show a popup — too noisy. Just make sure output is visible if user opens it.
    }
  }

  private parseLevel(str: string): LogLevel {
    switch (str.toLowerCase()) {
      case 'off': return LogLevel.Off;
      case 'error': return LogLevel.Error;
      case 'warn': case 'warning': return LogLevel.Warn;
      case 'info': return LogLevel.Info;
      case 'debug': return LogLevel.Debug;
      case 'trace': return LogLevel.Trace;
      default: return LogLevel.Info;
    }
  }
}
