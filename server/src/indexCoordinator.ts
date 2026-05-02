import { FileStore } from './fileStore';
import { WorkspaceIndex } from './workspaceIndex';
import { Connection, DiagnosticSeverity } from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { buildStoryDataResponseExported } from './handlers/queries';
import { normalizeUri } from './serverUtils';

// ---------------------------------------------------------------------------
// IndexCoordinator — single scheduler for all indexing work.
// ---------------------------------------------------------------------------

type ReadyCallback  = () => void;
type LogFn          = (msg: string) => void;

interface CoordinatorOptions {
  connection:  Connection;
  documents:   TextDocuments<TextDocument>;
  fileStore:   FileStore;
  workspace:   WorkspaceIndex;
  log:         LogFn;
  /** Debounce window in ms. Defaults to 120. */
  debounceMs?: number;
}

export class IndexCoordinator {
  private readonly connection: Connection;
  private readonly documents:  TextDocuments<TextDocument>;
  private readonly store:      FileStore;
  private readonly workspace:  WorkspaceIndex;
  private readonly log:        LogFn;
  private readonly debounceMs: number;

  private dirty    = new Set<string>();
  private timer:   ReturnType<typeof setTimeout> | null = null;
  private running  = false;
  private ready    = false;
  private readyCbs: ReadyCallback[] = [];

  constructor(opts: CoordinatorOptions) {
    this.connection  = opts.connection;
    this.documents   = opts.documents;
    this.store       = opts.fileStore;
    this.workspace   = opts.workspace;
    this.log         = opts.log;
    this.debounceMs  = opts.debounceMs ?? 120;
  }

  // ---- Public event API ---------------------------------------------------

  /** File content received from LSP (textDocument/didOpen or didChange). */
  onLspContent(uri: string, text: string, version: number): void {
    const normUri = normalizeUri(uri);
    const changed = this.store.upsert(normUri, text, 'lsp', version);
    if (changed) this.schedule(normUri);
  }

  /** File read from disk (initial scan or FS watcher for non-open files). */
  onDiskContent(uri: string, text: string): void {
    const normUri = normalizeUri(uri);
    // LSP content takes priority — don't overwrite open-document state.
    // Check both the normalised URI and any open document that normalises to it.
    if (this.documents.get(normUri) || this.documents.get(uri)) return;
    const changed = this.store.upsert(normUri, text, 'disk');
    if (changed) this.schedule(normUri);
  }

  /** File deleted (FS watcher or workspace/didChangeWatchedFiles). */
  onDelete(uri: string): void {
    const normUri = normalizeUri(uri);
    const existed = this.store.remove(normUri);
    this.workspace.removeFile(normUri);
    if (existed) {
      this.log(`[index] Removed ${normUri}`);
      this.scheduleAll();
    }
  }

  /** Force a full reanalysis of everything (e.g. after a refresh command). */
  scheduleAll(): void {
    for (const uri of this.store.uris()) this.dirty.add(uri);
    this.schedule();
  }

  /** Wait for the first completed analysis run. */
  onReady(cb: ReadyCallback): void {
    if (this.ready) { cb(); return; }
    this.readyCbs.push(cb);
  }

  // ---- Private scheduling -------------------------------------------------

  private schedule(uri?: string): void {
    if (uri) this.dirty.add(uri);
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => this.run(), this.debounceMs);
  }

  private async run(): Promise<void> {
    this.timer = null;
    if (this.running) {
      // Already running — re-schedule so we pick up any new dirtiness after
      // the current run finishes.  This prevents concurrent mutation of the
      // workspace index (e.g. upsert during reanalyzeAll) which can cause
      // stale or missing entries.
      this.timer = setTimeout(() => this.run(), this.debounceMs);
      return;
    }
    this.running = true;

    // Snapshot the dirty set and clear it ATOMICALLY so that any new
    // mutations arriving during the run go into a fresh dirty set and are
    // picked up by the next scheduled run, not interleaved into this one.
    const toProcess = [...this.dirty];
    this.dirty.clear();

    try {
      let parseErrors = 0;
      for (const uri of toProcess) {
        const text = this.store.getText(uri);
        if (text === undefined) continue;
        try {
          this.workspace.upsertFile(uri, text);
        } catch (err) {
          parseErrors++;
          this.log(`[index] Parse error for ${uri}: ${err}`);
        }
      }
      if (parseErrors > 0) {
        this.log(`[index] ${parseErrors} parse error(s) in this batch`);
      }

      this.workspace.reanalyzeAll();
      this.flushAllDiagnostics();
      // Build StoryData response once — use for both broadcast and format ID extraction
      const sd = buildStoryDataResponseExported(this.connection, this.documents, this.workspace);
      if (sd) {
        this.connection.sendNotification('knot/storyDataUpdated', sd);
        if (sd.format) this.workspace.setActiveFormatId(sd.format);
      }

      this.log(`[index] Reanalysis complete — ${this.workspace.getKnownUris().length} file(s) indexed`);

    } catch (err) {
      this.log(`[index] Reanalysis failed: ${err}`);
    } finally {
      this.running = false;
    }

    if (!this.ready) {
      this.ready = true;
      this.connection.sendNotification('knot/progressEnd', {});
      this.connection.sendNotification('knot/serverReady', {});
      for (const cb of this.readyCbs) cb();
      this.readyCbs = [];
    }

    if (this.dirty.size > 0) {
      this.schedule();
    }
  }

  // ---- Diagnostics ---------------------------------------------------------

  private flushAllDiagnostics(): void {
    const allUris = new Set([
      ...this.workspace.getKnownUris(),
      ...this.store.uris(),
    ]);

    for (const uri of allUris) {
      this.flushDiagnosticsForUri(uri);
    }
  }

  private flushDiagnosticsForUri(uri: string): void {
    const analysis = this.workspace.getAnalysis(uri);

    if (!analysis || analysis.diagnostics.length === 0) {
      this.connection.sendDiagnostics({ uri, diagnostics: [] });
      return;
    }

    const openDoc = this.documents.get(uri);
    if (openDoc) {
      this.connection.sendDiagnostics({
        uri,
        diagnostics: analysis.diagnostics.map(d => ({
          message:  d.message,
          range:    { start: openDoc.positionAt(d.range.start), end: openDoc.positionAt(d.range.end) },
          severity: d.severity === 'error' ? DiagnosticSeverity.Error : DiagnosticSeverity.Warning,
        })),
      });
      return;
    }

    const text = this.store.getText(uri);
    if (text) {
      this.connection.sendDiagnostics({
        uri,
        diagnostics: analysis.diagnostics.map(d => ({
          message:  d.message,
          range:    {
            start: offsetToLspPosition(text, d.range.start),
            end:   offsetToLspPosition(text, d.range.end),
          },
          severity: d.severity === 'error' ? DiagnosticSeverity.Error : DiagnosticSeverity.Warning,
        })),
      });
    }
  }
}

function offsetToLspPosition(text: string, offset: number): { line: number; character: number } {
  const slice = text.slice(0, Math.min(offset, text.length));
  const lines = slice.split('\n');
  return { line: lines.length - 1, character: lines[lines.length - 1]!.length };
}