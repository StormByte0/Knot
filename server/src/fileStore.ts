import * as crypto from 'node:crypto';

// ---------------------------------------------------------------------------
// FileStore — single source of truth for all indexed file content.
//
// Every file the server knows about lives here, regardless of whether it is
// currently open in an editor.  The `source` field records where the most
// recent content came from so callers can reason about freshness.
// ---------------------------------------------------------------------------

export type FileSource = 'lsp' | 'disk';

export interface StoredFile {
  uri:      string;
  text:     string;
  version:  number;   // LSP document version (0 for disk-sourced files)
  hash:     string;   // SHA-1 of text — used to skip no-op updates
  source:   FileSource;
  lastSeen: number;   // Date.now() of last upsert
}

export class FileStore {
  private files = new Map<string, StoredFile>();

  /**
   * Insert or update a file.  Returns true if the content actually changed
   * (hash differs), false if the upsert was a no-op.
   */
  upsert(uri: string, text: string, source: FileSource, version = 0): boolean {
    const hash    = sha1(text);
    const existing = this.files.get(uri);

    // LSP content always wins over disk content for the same URI.
    // Disk content only wins when there is no LSP version yet.
    if (existing) {
      if (existing.source === 'lsp' && source === 'disk') return false;
      if (existing.hash === hash) {
        this.files.set(uri, { ...existing, source, lastSeen: Date.now() });
        return false;
      }
    }

    this.files.set(uri, { uri, text, version, hash, source, lastSeen: Date.now() });
    return true;
  }

  remove(uri: string): boolean {
    return this.files.delete(uri);
  }

  get(uri: string): StoredFile | undefined {
    return this.files.get(uri);
  }

  getText(uri: string): string | undefined {
    return this.files.get(uri)?.text;
  }

  has(uri: string): boolean {
    return this.files.has(uri);
  }

  uris(): string[] {
    return [...this.files.keys()].sort();
  }

  size(): number {
    return this.files.size;
  }
}

function sha1(text: string): string {
  return crypto.createHash('sha1').update(text).digest('hex');
}