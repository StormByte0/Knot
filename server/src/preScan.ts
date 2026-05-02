import { Token, TokenType } from './tokenTypes';

export interface MacroPairTable {
  // key: range.start of MacroOpen token
  // value: range.start of matching MacroCloseOpen, or null if no <</name>> found
  pairs: Map<number, number | null>;
  // MacroOpen offsets that are genuinely unclosed block macros.
  // This is the set of macros still on the name-stack at EOF that also lack
  // a corresponding MacroClose >> (i.e. their arg list was never closed either).
  // Macros whose >> was found but whose <</name>> was not found are NOT in this
  // set — they are simply self-closing.
  unclosed: Set<number>;
  // Orphaned close tags (no matching open) — emit diagnostics for these
  orphans: Token[];
}

export function preScan(tokens: Token[]): MacroPairTable {
  const stack: Array<{ name: string; openOffset: number }> = [];
  const pairs = new Map<number, number | null>();
  const orphans: Token[] = [];
  // Track which MacroOpen offsets have seen their MacroClose >>
  const argsClosed = new Set<number>();

  // We need to pair MacroOpen with their MacroClose >> too.
  // Walk through tracking nesting for arg close detection.
  // MacroOpen pushes to argStack; MacroClose >> pops the top.
  const argStack: number[] = []; // stack of MacroOpen offsets

  for (let i = 0; i < tokens.length; i++) {
    const tok = tokens[i]!;

    if (tok.type === TokenType.MacroOpen) {
      const maybeName = tokens[i + 1];
      const name = maybeName?.type === TokenType.MacroName ? maybeName.value : '';
      stack.push({ name, openOffset: tok.range.start });
      argStack.push(tok.range.start);
      pairs.set(tok.range.start, null);
      continue;
    }

    if (tok.type === TokenType.MacroClose && argStack.length > 0) {
      // The >> that closes the most recent macro's arg list
      const openOffset = argStack.pop()!;
      argsClosed.add(openOffset);
      continue;
    }

    if (tok.type !== TokenType.MacroCloseOpen) continue;

    const maybeCloseName = tokens[i + 1];
    const closeName = maybeCloseName?.type === TokenType.MacroName ? maybeCloseName.value : '';

    let matched = false;
    for (let j = stack.length - 1; j >= 0; j--) {
      const open = stack[j]!;
      if (open.name !== closeName) continue;
      pairs.set(open.openOffset, tok.range.start);
      stack.splice(j);
      matched = true;
      break;
    }

    if (!matched) orphans.push(tok);
  }

  // Macros still on the name-stack at EOF are unmatched by <</name>>.
  // But if their args >> was found, they're just self-closing — no error.
  // If their args >> was NOT found, the macro is truncated/malformed — error.
  const unclosed = new Set(
    stack
      .filter(s => !argsClosed.has(s.openOffset))
      .map(s => s.openOffset)
  );

  return { pairs, unclosed, orphans };
}
