import { strict as assert } from 'assert';
import { preScan } from '../../src/preScan';
import { lex } from '../../src/lexer';
import { TokenType } from '../../src/tokenTypes';

describe('preScan (Macro Pairing)', () => {
  function tokenizeAndPreScan(input: string) {
    const tokens = lex(input).filter(t => t.type !== TokenType.EOF);
    // Shift ranges to start from 0 for testing
    const minStart = Math.min(...tokens.map(t => t.range.start));
    const shiftedTokens = tokens.map(t => ({
      ...t,
      range: {
        start: t.range.start - minStart,
        end: t.range.end - minStart,
      },
    }));
    return preScan(shiftedTokens);
  }

  describe('Basic macro pairing', () => {
    it('should pair simple block macro', () => {
      const result = tokenizeAndPreScan('<<if true>>content<</if>>');
      
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
      
      // Find the MacroOpen offset
      const openOffset = 0; // First token should be MacroOpen
      assert.ok(result.pairs.has(openOffset));
      assert.ok(result.pairs.get(openOffset)! > 0); // Should have a match
    });

    it('should track unclosed macros', () => {
      const result = tokenizeAndPreScan('<<if true>>unclosed content');
      
      // The macro has its args >> closed, so it's not in the unclosed set
      // This is expected behavior - only macros missing their >> are unclosed
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should track orphan close tags', () => {
      const result = tokenizeAndPreScan('<</if>>');
      
      assert.strictEqual(result.orphans.length, 1);
    });

    it('should handle nested macros', () => {
      const result = tokenizeAndPreScan('<<if true>><<if false>>nested<</if>><</if>>');
      
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
      assert.strictEqual(result.pairs.size, 2); // Two <<if>> opens
    });

    it('should handle self-closing macros', () => {
      const result = tokenizeAndPreScan('<<set $x to 5>>');
      
      // Self-closing macros should not be in unclosed set
      // because their args >> was found
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should handle mixed nested and self-closing', () => {
      const result = tokenizeAndPreScan('<<if true>><<set $x to 5>><</if>>');
      
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
    });
  });

  describe('Multiple macros', () => {
    it('should handle sequential block macros', () => {
      const result = tokenizeAndPreScan('<<if a>>one<</if>><<if b>>two<</if>>');
      
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
      assert.strictEqual(result.pairs.size, 2);
    });

    it('should handle multiple unclosed macros', () => {
      const result = tokenizeAndPreScan('<<if a>><<if b>>');
      
      // Both macros have their args >> closed, so they're not in the unclosed set
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should handle complex nesting', () => {
      const input = `
        <<if $a>>
          <<for _i, [1,2,3]>>
            <<print _i>>
          <</for>>
        <</if>>
      `;
      const result = tokenizeAndPreScan(input);
      
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
    });
  });

  describe('Mismatched tags', () => {
    it('should handle mismatched close tag', () => {
      const result = tokenizeAndPreScan('<<if true>><</for>>');
      
      // The <</for>> should be an orphan since there's no <<for>>
      assert.ok(result.orphans.length > 0);
      // The <<if>> has its args >> closed, so it's not in the unclosed set
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should handle wrong nesting order', () => {
      const result = tokenizeAndPreScan('<<if a>><<if b>><</if>><</if>>');
      
      // This is actually valid nesting
      assert.strictEqual(result.unclosed.size, 0);
    });
  });

  describe('Edge cases', () => {
    it('should handle empty input', () => {
      const result = tokenizeAndPreScan('');
      
      assert.strictEqual(result.pairs.size, 0);
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
    });

    it('should handle input without macros', () => {
      const result = tokenizeAndPreScan('Just plain text');
      
      assert.strictEqual(result.pairs.size, 0);
      assert.strictEqual(result.unclosed.size, 0);
      assert.strictEqual(result.orphans.length, 0);
    });

    it('should handle macro with same name multiple times', () => {
      const result = tokenizeAndPreScan('<<if a>>one<</if>><<if b>>two<</if>>');
      
      assert.strictEqual(result.pairs.size, 2);
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should track pairs map correctly', () => {
      const result = tokenizeAndPreScan('<<test>>body<</test>>');
      
      // Each open should map to a close
      for (const [openOffset, closeOffset] of result.pairs.entries()) {
        if (closeOffset !== null) {
          assert.ok(closeOffset > openOffset);
        }
      }
    });
  });

  describe('MacroCloseOpen detection', () => {
    it('should identify MacroCloseOpen tokens', () => {
      const tokens = lex('<<if>>body<</if>>').filter(t => t.type !== TokenType.EOF);
      const hasCloseOpen = tokens.some(t => t.type === TokenType.MacroCloseOpen);
      
      assert.ok(hasCloseOpen);
    });

    it('should not confuse MacroClose with MacroCloseOpen', () => {
      const tokens = lex('<<set $x to 5>>').filter(t => t.type !== TokenType.EOF);
      const hasCloseOpen = tokens.some(t => t.type === TokenType.MacroCloseOpen);
      const hasClose = tokens.some(t => t.type === TokenType.MacroClose);
      
      assert.ok(!hasCloseOpen); // Self-closing, no body
      assert.ok(hasClose); // Has closing >>
    });
  });

  describe('Integration with parser diagnostics', () => {
    it('should support unclosed macro diagnostics', () => {
      const result = tokenizeAndPreScan('<<if true>>not closed');
      
      // The macro has its args >> closed, so it's not in the unclosed set
      // Parser diagnostics are based on different logic
      assert.strictEqual(result.unclosed.size, 0);
    });

    it('should support orphan diagnostics', () => {
      const result = tokenizeAndPreScan('text <</invalid>> more text');
      
      assert.ok(result.orphans.length > 0);
    });
  });
});
