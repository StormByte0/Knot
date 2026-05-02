import type { CompletionItem, Diagnostic } from 'vscode-languageserver/node';
import type {
  StoryFormatAdapter,
  FormatContext,
  AdapterCompletionRequest,
  AdapterHoverRequest,
  AdapterDiagnosticRequest,
} from '../types';

// ---------------------------------------------------------------------------
// FallbackAdapter
//
// Used when the active story format is unknown or not yet supported.
// All methods return safe empty values — no SugarCube-specific behaviour.
// Users will see basic workspace features (passage nav, go-to-definition on
// user variables) but no format-specific completions or hover docs.
// ---------------------------------------------------------------------------

export class FallbackAdapter implements StoryFormatAdapter {
  readonly id          = 'fallback';
  readonly displayName = 'Unknown Format';

  provideFormatCompletions(_req: AdapterCompletionRequest, _ctx: FormatContext): CompletionItem[] {
    return [];
  }

  buildMacroSnippet(_name: string, _hasBody: boolean): string | null {
    return null;
  }

  getBlockMacroNames(): ReadonlySet<string> {
    return new Set();
  }

  provideBuiltinHover(_req: AdapterHoverRequest, _ctx: FormatContext): string | null {
    return null;
  }

  describeVariableSigil(_sigil: string): string | null {
    return null;
  }

  provideDiagnostics(_req: AdapterDiagnosticRequest, _ctx: FormatContext): Diagnostic[] {
    return [];
  }

  getVirtualRuntimePrelude(): string {
    return '';
  }
}