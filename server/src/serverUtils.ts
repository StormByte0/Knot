import * as fs from 'node:fs';
import * as path from 'node:path';
import { Location, Range } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { InferredType } from './typeInference';
import { SourceRange } from './tokenTypes';

// ---------------------------------------------------------------------------
// URI ↔ path conversion
//
// Windows quirk: VS Code always uses lowercase drive letters in file: URIs
// (e.g. file:///c:/foo).  Node's fs APIs return paths with the casing the
// filesystem gives back, which on Windows is typically uppercase (C:\foo).
// pathToUri normalises the drive letter to lowercase so that URIs produced
// by the disk scanner match the URIs sent by the LSP client, preventing the
// same file from being indexed twice under two different URI keys.
// ---------------------------------------------------------------------------

/**
 * Normalise a file: URI to a canonical form so that URIs produced by the
 * disk scanner (via pathToUri) and URIs sent by the LSP client always match.
 *
 * On Windows, VS Code always uses a lowercase drive letter (file:///c:/...)
 * while Node's fs APIs can return uppercase (C:\...).  We lowercase the
 * drive letter and percent-decode the path so both sides hash identically.
 */
export function normalizeUri(uri: string): string {
  if (!uri.startsWith('file:')) return uri;
  // Decode any percent-encoding so file:///C%3A/... and file:///C:/... match
  let decoded = uri;
  try { decoded = decodeURIComponent(uri); } catch { /* leave as-is */ }
  // Lowercase the drive letter: file:///C:/ → file:///c:/
  return decoded.replace(/^(file:\/\/\/)([A-Z]:)/, (_, scheme, drive) =>
    scheme + drive.toLowerCase(),
  );
}

export function uriToPath(uri: string): string {
  // Decode percent-encoding first (%3A -> :, %20 -> space, etc.)
  let p = decodeURIComponent(uri);
  // Strip scheme + authority
  p = p.replace(/^file:\/\/\//, '/').replace(/^file:\/\//, '');
  // On Windows: /c:/foo -> c:/foo  (already lowercase from VS Code)
  p = p.replace(/^\/([A-Za-z]:[\/])/, '$1');
  // Normalise forward slashes
  return p;
}

export function pathToUri(p: string): string {
  const normalized = p.replace(/\\/g, '/');
  // Windows absolute path: normalise drive letter to lowercase to match VS Code
  if (/^[A-Za-z]:/.test(normalized)) {
    const lowered = normalized[0]!.toLowerCase() + normalized.slice(1);
    return `file:///${lowered}`;
  }
  return `file://${normalized.startsWith('/') ? '' : '/'}${normalized}`;
}

export function offsetToPosition(text: string, offset: number): { line: number; character: number } {
  const slice = text.slice(0, Math.min(offset, text.length));
  const lines = slice.split('\n');
  return { line: lines.length - 1, character: lines[lines.length - 1]!.length };
}

export function getFileText(uri: string, documents: { get(uri: string): TextDocument | undefined }, fileStore?: { getText(uri: string): string | undefined }): string | null {
  const openDoc = documents.get(uri);
  if (openDoc) return openDoc.getText();
  // Check file store before falling back to disk
  if (fileStore) {
    const cached = fileStore.getText(uri);
    if (cached !== undefined) return cached;
  }
  try { return fs.readFileSync(uriToPath(uri), 'utf-8'); }
  catch { return null; }
}

export function defToLocation(def: { uri: string; range: SourceRange }, documents: { get(uri: string): TextDocument | undefined }, fileStore?: { getText(uri: string): string | undefined }): Location {
  const openDoc = documents.get(def.uri);
  if (openDoc) {
    return Location.create(def.uri, Range.create(openDoc.positionAt(def.range.start), openDoc.positionAt(def.range.end)));
  }
  // Try file store before disk I/O
  const text = fileStore?.getText(def.uri) ?? (() => { try { return fs.readFileSync(uriToPath(def.uri), 'utf-8'); } catch { return null; } })();
  if (text) {
    return Location.create(def.uri, Range.create(offsetToPosition(text, def.range.start), offsetToPosition(text, def.range.end)));
  }
  return Location.create(def.uri, Range.create(0, 0, 0, 0));
}

export function refToLocation(ref: { uri: string; range: SourceRange }, documents: { get(uri: string): TextDocument | undefined }, fileStore?: { getText(uri: string): string | undefined }): Location {
  return defToLocation(ref, documents, fileStore);
}

export function inferredTypeToString(t: InferredType): string {
  switch (t.kind) {
    case 'object': return 'object';
    case 'array':  return t.elements ? `${t.elements.kind}[]` : 'array';
    default:       return t.kind;
  }
}

export function resolveTypePath(type: InferredType, segments: string[]): InferredType | null {
  let cur: InferredType = type;
  for (const key of segments) {
    if (cur.kind !== 'object' || !cur.properties) return null;
    const next = cur.properties[key];
    if (!next) return null;
    cur = next;
  }
  return cur;
}

export function buildTypeSection(t: InferredType): string {
  if (t.kind === 'object' && t.properties) {
    const entries = Object.entries(t.properties);
    if (entries.length === 0) return '\n\n**Type:** `object`';
    if (entries.length <= 20) {
      const rows = entries.map(([k, v]) => `| \`.${k}\` | \`${inferredTypeToString(v)}\` |`).join('\n');
      return `\n\n| Property | Type |\n|---|---|\n${rows}`;
    }
    return `\n\n**Type:** \`object\`\n\n*${entries.length} properties*`;
  }
  if (t.kind === 'array') return `\n\n**Type:** \`${t.elements ? inferredTypeToString(t.elements) : 'unknown'}[]\``;
  return `\n\n**Type:** \`${inferredTypeToString(t)}\``;
}

export function wordAt(text: string, offset: number): string {
  // Search locally around the offset instead of scanning from position 0
  const start = Math.max(0, text.lastIndexOf(' ', offset) + 1);
  const m = text.slice(start).match(/^([$A-Za-z_][A-Za-z0-9_]*)/);
  return m && start + m[0].length >= offset ? m[0] : '';
}

export function findWordStart(text: string, offset: number, word: string): number {
  let pos = offset;
  while (pos >= 0) {
    const idx = text.lastIndexOf(word, pos);
    if (idx === -1) break;
    if (idx <= offset && idx + word.length >= offset) return idx;
    pos = idx - 1;
  }
  return -1;
}