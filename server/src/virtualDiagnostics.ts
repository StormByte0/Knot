import * as acorn from 'acorn';
import { DocumentNode, ParseDiagnostic } from './ast';
import { VirtualDocGenerator } from './virtualDoc';

export interface VirtualDiagnosticResult {
  diagnostics: ParseDiagnostic[];
  virtualContent: string;
}

export function runVirtualDiagnostics(ast: DocumentNode, uri: string): VirtualDiagnosticResult {
  const generator = new VirtualDocGenerator();
  const virtual = generator.generate(ast, uri);
  const diagnostics: ParseDiagnostic[] = [];

  try {
    acorn.parse(virtual.content, { ecmaVersion: 'latest' });
  } catch (error) {
    if (isAcornError(error)) {
      const mapped = generator.mapOffsetToOriginal(virtual, error.pos ?? 0);
      if (mapped !== null) {
        diagnostics.push({
          message: `Virtual JS error: ${error.message}`,
          range: { start: mapped, end: mapped + 1 },
        });
      }
    }
  }

  return { diagnostics, virtualContent: virtual.content };
}

function isAcornError(error: unknown): error is Error & { pos?: number } {
  if (!error || typeof error !== 'object') return false;
  return 'message' in error;
}
