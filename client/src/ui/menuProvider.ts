/**
 * Knot v2 — Menu Provider
 *
 * Provides a quick-pick menu for all Knot commands.
 * Serves as the main entry point for the knot.mainMenu command.
 */

import * as vscode from 'vscode';
import { LanguageClient } from 'vscode-languageclient/node';

export class MenuProvider {
  private client: LanguageClient;

  constructor(client: LanguageClient) {
    this.client = client;
  }

  /** Show the main Knot command menu as a quick pick. */
  async showMainMenu(): Promise<void> {
    const commands = [
      { label: '$(globe) Go to Passage', command: 'knot.goToPassage', description: 'Navigate to a passage by name' },
      { label: '$(build) Build', command: 'knot.build', description: 'Build project with Tweego' },
      { label: '$(beaker) Build (Test Mode)', command: 'knot.buildTest', description: 'Build in test mode' },
      { label: '$(eye-watch) Start Watch Mode', command: 'knot.startWatch', description: 'Auto-rebuild on file changes' },
      { label: '$(primitive-square) Stop Watch Mode', command: 'knot.stopWatch', description: 'Stop auto-rebuild' },
      { label: '$(book) List Story Formats', command: 'knot.listFormats', description: 'Show available story formats' },
      { label: '$(bookmark) Select Story Format', command: 'knot.selectFormat', description: 'Change active story format' },
      { label: '$(refresh) Refresh Documents', command: 'knot.refreshDocuments', description: 'Re-index all workspace documents' },
      { label: '$(verified) Verify Tweego', command: 'knot.verifyTweego', description: 'Check Tweego installation' },
      { label: '$(output) Show Server Output', command: 'knot.showOutput', description: 'Open language server output channel' },
      { label: '$(refresh) Restart Server', command: 'knot.restart', description: 'Restart the language server' },
      { label: '$(settings-gear) Open Settings', command: 'knot.openSettings', description: 'Open Knot settings' },
    ];

    const picked = await vscode.window.showQuickPick(commands, {
      placeHolder: 'Knot — Select a command...',
    });

    if (picked) {
      vscode.commands.executeCommand(picked.command);
    }
  }
}
