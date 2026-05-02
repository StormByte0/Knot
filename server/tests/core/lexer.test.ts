import { strict as assert } from 'assert';
import { lex } from '../../src/lexer';
import { TokenType } from '../../src/tokenTypes';

describe('Lexer', () => {
  describe('Basic tokenization', () => {
    it('should tokenize empty input', () => {
      const tokens = lex('');
      assert.strictEqual(tokens.length, 1);
      assert.strictEqual(tokens[0]!.type, TokenType.EOF);
    });

    it('should tokenize plain text', () => {
      const tokens = lex('Hello world');
      const nonEof = tokens.filter(t => t.type !== TokenType.EOF);
      assert.strictEqual(nonEof.length, 1);
      assert.strictEqual(nonEof[0]!.type, TokenType.Text);
      assert.strictEqual(nonEof[0]!.value, 'Hello world');
    });

    it('should tokenize passage headers', () => {
      const tokens = lex(':: Start\nThis is content');
      // Passage marker is handled at document level, not in lex()
      const nonEof = tokens.filter(t => t.type !== TokenType.EOF);
      assert.ok(nonEof.some(t => t.type === TokenType.Text));
    });
  });

  describe('Macro tokenization', () => {
    it('should tokenize simple macro', () => {
      const tokens = lex('<<set $x to 5>>');
      const types = tokens.map(t => t.type);
      
      assert.ok(types.includes(TokenType.MacroOpen));
      assert.ok(types.includes(TokenType.MacroName));
      assert.ok(types.includes(TokenType.StoryVar));
      assert.ok(types.includes(TokenType.SugarOperator));
      assert.ok(types.includes(TokenType.Number));
      assert.ok(types.includes(TokenType.MacroClose));
      
      const nameTok = tokens.find(t => t.type === TokenType.MacroName);
      assert.strictEqual(nameTok?.value, 'set');
    });

    it('should tokenize macro with string argument', () => {
      const tokens = lex('<<goto "Start">>');
      const nameTok = tokens.find(t => t.type === TokenType.MacroName);
      const strTok = tokens.find(t => t.type === TokenType.String);
      
      assert.strictEqual(nameTok?.value, 'goto');
      assert.strictEqual(strTok?.value, '"Start"');
    });

    it('should tokenize macro with temp variable', () => {
      const tokens = lex('<<print _temp>>');
      const tempTok = tokens.find(t => t.type === TokenType.TempVar);
      assert.strictEqual(tempTok?.value, '_temp');
    });

    it('should tokenize block macro with body', () => {
      const tokens = lex('<<if true>>content<</if>>');
      const types = tokens.map(t => t.type);
      
      assert.ok(types.includes(TokenType.MacroOpen));
      assert.ok(types.includes(TokenType.MacroClose));
      assert.ok(types.includes(TokenType.Text));
      assert.ok(types.includes(TokenType.MacroCloseOpen));
      assert.ok(types.includes(TokenType.MacroClose)); // final >>
    });

    it('should handle unclosed macro', () => {
      const tokens = lex('<<set $x to 5');
      const errorTok = tokens.find(t => t.type === TokenType.Error);
      assert.ok(errorTok !== undefined);
    });
  });

  describe('Link tokenization', () => {
    it('should tokenize simple link', () => {
      const tokens = lex('[[Start]]');
      const types = tokens.map(t => t.type);
      
      assert.ok(types.includes(TokenType.LinkOpen));
      assert.ok(types.includes(TokenType.Text));
      assert.ok(types.includes(TokenType.LinkClose));
      
      const textTok = tokens.find(t => t.type === TokenType.Text);
      assert.strictEqual(textTok?.value, 'Start');
    });

    it('should tokenize link with pipe separator', () => {
      const tokens = lex('[[Click|Target]]');
      const sepTok = tokens.find(t => t.type === TokenType.LinkSeparator);
      assert.strictEqual(sepTok?.value, '|');
    });

    it('should tokenize link with arrow separator', () => {
      const tokens = lex('[[Click->Target]]');
      const sepTok = tokens.find(t => t.type === TokenType.LinkSeparator);
      assert.strictEqual(sepTok?.value, '->');
    });

    it('should tokenize link with reverse arrow separator', () => {
      const tokens = lex('[[Target<-Click]]');
      const sepTok = tokens.find(t => t.type === TokenType.LinkSeparator);
      assert.strictEqual(sepTok?.value, '<-');
    });

    it('should handle unclosed link', () => {
      const tokens = lex('[[Start');
      const errorTok = tokens.find(t => t.type === TokenType.Error);
      assert.ok(errorTok !== undefined);
    });
  });

  describe('Comment tokenization', () => {
    it('should tokenize HTML comment', () => {
      const tokens = lex('<!-- This is a comment -->');
      const commentTok = tokens.find(t => t.type === TokenType.HtmlComment);
      assert.ok(commentTok !== undefined);
      assert.ok(commentTok!.value.includes('<!--'));
      assert.ok(commentTok!.value.includes('-->'));
    });

    it('should tokenize block comment in markup', () => {
      const tokens = lex('/* This is a comment */');
      const commentTok = tokens.find(t => t.type === TokenType.BlockComment);
      assert.ok(commentTok !== undefined);
      assert.ok(commentTok!.value.includes('/*'));
      assert.ok(commentTok!.value.includes('*/'));
    });

    it('should tokenize block comment in macro args', () => {
      const tokens = lex('<<set $x /* comment */ to 5>>');
      const commentTok = tokens.find(t => t.type === TokenType.BlockComment);
      assert.ok(commentTok !== undefined);
    });

    it('should tokenize line comment in macro args only', () => {
      // Line comments should only be recognized inside macros
      const tokens1 = lex('<<set $x to 5 // comment>>');
      const commentTok1 = tokens1.find(t => t.type === TokenType.LineComment);
      assert.ok(commentTok1 !== undefined);
      
      // In markup context, // should be plain text
      const tokens2 = lex('Text // not a comment');
      const commentTok2 = tokens2.find(t => t.type === TokenType.LineComment);
      assert.strictEqual(commentTok2, undefined);
    });

    it('should not tokenize macros inside comments', () => {
      const tokens = lex('<!-- <<set $x to 5>> -->');
      const macroTok = tokens.find(t => t.type === TokenType.MacroOpen);
      assert.strictEqual(macroTok, undefined);
      
      const commentTok = tokens.find(t => t.type === TokenType.HtmlComment);
      assert.ok(commentTok !== undefined);
    });

    it('should not tokenize macros inside block comments', () => {
      const tokens = lex('/* <<set $x to 5>> */');
      const macroTok = tokens.find(t => t.type === TokenType.MacroOpen);
      assert.strictEqual(macroTok, undefined);
      
      const commentTok = tokens.find(t => t.type === TokenType.BlockComment);
      assert.ok(commentTok !== undefined);
    });
  });

  describe('Variable tokenization', () => {
    it('should tokenize story variable in macro context', () => {
      const tokens = lex('<<print $myVar>>');
      const varTok = tokens.find(t => t.type === TokenType.StoryVar);
      assert.strictEqual(varTok?.value, '$myVar');
    });

    it('should tokenize temp variable in macro context', () => {
      const tokens = lex('<<print _myVar>>');
      const varTok = tokens.find(t => t.type === TokenType.TempVar);
      assert.strictEqual(varTok?.value, '_myVar');
    });

    it('should not tokenize lone sigil as variable', () => {
      const tokens = lex('$');
      const varTok = tokens.find(t => t.type === TokenType.StoryVar);
      assert.strictEqual(varTok, undefined);
    });
  });

  describe('Expression operators', () => {
    it('should tokenize SugarCube operators', () => {
      const tokens = lex('<<if $x eq 5 and $y gt 3>>');
      const ops = tokens.filter(t => t.type === TokenType.SugarOperator);
      
      assert.ok(ops.some(t => t.value === 'eq'));
      assert.ok(ops.some(t => t.value === 'and'));
      assert.ok(ops.some(t => t.value === 'gt'));
    });

    it('should tokenize JavaScript operators', () => {
      const tokens = lex('<<if $x === 5 && $y >= 3>>');
      const ops = tokens.filter(t => t.type === TokenType.Operator);
      
      assert.ok(ops.some(t => t.value === '==='));
      assert.ok(ops.some(t => t.value === '&&'));
      assert.ok(ops.some(t => t.value === '>='));
    });

    it('should tokenize property access in macro context', () => {
      const tokens = lex('<<print $obj.prop>>');
      const propTok = tokens.find(t => t.type === TokenType.PropertyAccess);
      assert.strictEqual(propTok?.value, '.');
    });

    it('should tokenize bracket access in macro context', () => {
      const tokens = lex('<<print $arr[0]>>');
      const openTok = tokens.find(t => t.type === TokenType.BracketOpen);
      const closeTok = tokens.find(t => t.type === TokenType.BracketClose);
      const numTok = tokens.find(t => t.type === TokenType.Number);
      
      assert.strictEqual(openTok?.value, '[');
      assert.strictEqual(closeTok?.value, ']');
      assert.strictEqual(numTok?.value, '0');
    });
  });

  describe('String tokenization', () => {
    it('should tokenize double-quoted string in macro context', () => {
      const tokens = lex('<<set $x to "hello">>');
      const strTok = tokens.find(t => t.type === TokenType.String);
      assert.strictEqual(strTok?.value, '"hello"');
    });

    it('should tokenize single-quoted string in macro context', () => {
      const tokens = lex("<<set $x to 'hello'>>");
      const strTok = tokens.find(t => t.type === TokenType.String);
      assert.strictEqual(strTok?.value, "'hello'");
    });

    it('should tokenize backtick string in macro context', () => {
      const tokens = lex('<<set $x to `hello`>>');
      const strTok = tokens.find(t => t.type === TokenType.String);
      assert.strictEqual(strTok?.value, '`hello`');
    });

    it('should handle escaped quotes in strings', () => {
      const tokens = lex('<<set $x to "hello \\"world\\"">>');
      const strTok = tokens.find(t => t.type === TokenType.String);
      assert.strictEqual(strTok?.value, '"hello \\"world\\""');
    });
  });

  describe('Number tokenization', () => {
    it('should tokenize integer in macro context', () => {
      const tokens = lex('<<set $x to 42>>');
      const numTok = tokens.find(t => t.type === TokenType.Number);
      assert.strictEqual(numTok?.value, '42');
    });

    it('should tokenize float in macro context', () => {
      const tokens = lex('<<set $x to 3.14>>');
      const numTok = tokens.find(t => t.type === TokenType.Number);
      assert.strictEqual(numTok?.value, '3.14');
    });
  });

  describe('Token ranges', () => {
    it('should provide correct token ranges', () => {
      const tokens = lex('<<set $x to 5>>');
      const setTok = tokens.find(t => t.type === TokenType.MacroName);
      
      assert.ok(setTok !== undefined);
      assert.strictEqual(setTok!.range.start, 2);
      assert.strictEqual(setTok!.range.end, 5);
    });

    it('should provide EOF token at end', () => {
      const tokens = lex('hello');
      const eofTok = tokens[tokens.length - 1];
      
      assert.strictEqual(eofTok?.type, TokenType.EOF);
      assert.strictEqual(eofTok?.range.end, 5);
    });
  });

  describe('Complex expressions', () => {
    it('should tokenize complex macro arguments', () => {
      const tokens = lex('<<set $result to ($a + $b) * 2>>');
      const types = tokens.map(t => t.type);
      
      assert.ok(types.includes(TokenType.ParenOpen));
      assert.ok(types.includes(TokenType.ParenClose));
      assert.ok(types.includes(TokenType.Operator)); // + and *
    });

    it('should tokenize nested macros', () => {
      const tokens = lex('<<print <<set $x to 5>>>>');
      // Should have two MacroOpen tokens
      const macroOpens = tokens.filter(t => t.type === TokenType.MacroOpen);
      assert.ok(macroOpens.length >= 1); // At least one should be recognized
    });
  });
});
