import { DocumentNode } from './ast';
import type { StoryFormatAdapter } from './formats/types';

// ---------------------------------------------------------------------------
// StoryData is a special passage whose body is raw JSON.
// Tweego reads it to determine the story format, IFID, and starting passage.
// We parse it so the LSP can:
//   - Warn when IFID is missing
//   - Validate the starting passage exists
//   - Surface the format name in the status bar
//   - Gate macro completions on the declared format (future)
// ---------------------------------------------------------------------------

export interface StoryData {
  ifid: string | null;
  format: string | null;
  formatVersion: string | null;
  start: string | null;
  raw: Record<string, unknown>;
}

const EMPTY: StoryData = {
  ifid: null,
  format: null,
  formatVersion: null,
  start: null,
  raw: {},
};

export function parseStoryData(ast: DocumentNode, adapter?: StoryFormatAdapter): StoryData {
  const sdName = adapter?.getStoryDataPassageName();
  const passage = sdName ? ast.passages.find(p => p.name === sdName) : undefined;
  if (!passage) return EMPTY;

  // The body of StoryData is either a raw ScriptBodyNode (source string)
  // or a MarkupNode array of text nodes — either way we reconstruct the
  // source string and JSON.parse it.
  let source = '';
  if (!Array.isArray(passage.body)) {
    // scriptBody or styleBody
    source = 'source' in passage.body ? passage.body.source : '';
  } else {
    // markup body — concatenate all text nodes
    for (const node of passage.body) {
      if (node.type === 'text') source += node.value;
    }
  }

  source = source.trim();
  if (!source) return EMPTY;

  let raw: Record<string, unknown> = {};
  try {
    const parsed = JSON.parse(source);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      raw = parsed as Record<string, unknown>;
    }
  } catch {
    // Malformed JSON — still return what we can
    return EMPTY;
  }

  return {
    ifid:          typeof raw['ifid'] === 'string' ? raw['ifid'] : null,
    format:        typeof raw['format'] === 'string' ? raw['format'] : null,
    formatVersion: typeof raw['format-version'] === 'string' ? raw['format-version'] : null,
    start:         typeof raw['start'] === 'string' ? raw['start'] : null,
    raw,
  };
}

// ---------------------------------------------------------------------------
// Diagnostics helpers — used by the analyzer or LSP server
// ---------------------------------------------------------------------------

export interface StoryDataDiagnostic {
  message: string;
  severity: 'error' | 'warning' | 'info';
  // Range inside the StoryData passage body; null means highlight the header
  rangeHint: null;
}

export function validateStoryData(
  data: StoryData,
  knownPassageNames: Set<string>,
): StoryDataDiagnostic[] {
  const diags: StoryDataDiagnostic[] = [];

  if (!data.ifid) {
    diags.push({
      message: 'StoryData is missing an "ifid" field. Tweego will fail to compile without one.',
      severity: 'error',
      rangeHint: null,
    });
  } else if (!isValidIfid(data.ifid)) {
    diags.push({
      message: `StoryData "ifid" value "${data.ifid}" is not a valid UUID v4.`,
      severity: 'warning',
      rangeHint: null,
    });
  }

  if (data.start && !knownPassageNames.has(data.start)) {
    diags.push({
      message: `StoryData "start" passage "${data.start}" does not exist in the workspace.`,
      severity: 'error',
      rangeHint: null,
    });
  }

  return diags;
}

// UUID v4 format validation
function isValidIfid(ifid: string): boolean {
  return /^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-4[0-9A-Fa-f]{3}-[89ABab][0-9A-Fa-f]{3}-[0-9A-Fa-f]{12}$/.test(ifid);
}