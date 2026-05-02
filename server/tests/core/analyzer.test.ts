import { strict as assert } from 'assert';
import { SyntaxAnalyzer } from '../../src/analyzer';
import { parseDocument } from '../../src/parser';
import { WorkspaceIndex } from '../../src/workspaceIndex';

describe('SyntaxAnalyzer', () => {
  let analyzer: SyntaxAnalyzer;
  let workspace: WorkspaceIndex;

  beforeEach(() => {
    analyzer = new SyntaxAnalyzer();
    workspace = new WorkspaceIndex();
  });

  describe('Link validation', () => {
    it('should report unknown passage links', () => {
      const { ast } = parseDocument(':: Start\n[[Unknown]]');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const linkDiags = result.diagnostics.filter(d => d.message.includes('Unknown passage'));
      assert.ok(linkDiags.length > 0);
    });

    it('should not report known passage links', () => {
      workspace.upsertFile('test://file1.tw', ':: Target\nContent');
      workspace.reanalyzeAll();
      
      const { ast } = parseDocument(':: Start\n[[Target]]');
      const result = analyzer.analyze(ast, 'test://test2.tw', workspace);
      
      const linkDiags = result.diagnostics.filter(d => d.message.includes('Unknown passage'));
      assert.strictEqual(linkDiags.length, 0);
    });

    it('should resolve links within same document', () => {
      const { ast } = parseDocument(':: Start\n[[Next]]\n\n:: Next\nContent');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const linkDiags = result.diagnostics.filter(d => d.message.includes('Unknown passage'));
      assert.strictEqual(linkDiags.length, 0);
    });
  });

  describe('Macro validation', () => {
    it('should report unknown macros', () => {
      const { ast } = parseDocument(':: Start\n<<unknownMacro>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const macroDiags = result.diagnostics.filter(d => d.message.includes('Unknown macro'));
      assert.ok(macroDiags.length > 0);
    });

    it('should not report builtin SugarCube macros', () => {
      const { ast } = parseDocument(':: Start\n<<if true>>content<</if>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const macroDiags = result.diagnostics.filter(d => d.message.includes('Unknown macro'));
      assert.strictEqual(macroDiags.length, 0);
    });

    it('should not report custom macros registered in workspace', () => {
      workspace.upsertFile('test://macros.tw', ':: StoryInit\n<<widget "myWidget">>content<</widget>>');
      workspace.reanalyzeAll();
      
      const { ast } = parseDocument(':: Start\n<<myWidget>>');
      const result = analyzer.analyze(ast, 'test://test2.tw', workspace);
      
      const macroDiags = result.diagnostics.filter(d => d.message.includes('Unknown macro'));
      assert.strictEqual(macroDiags.length, 0);
    });
  });

  describe('Type checking', () => {
    it('should report type mismatch in comparison operators', () => {
      const { ast } = parseDocument(':: Start\n<<if "string" gt 5>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const typeDiags = result.diagnostics.filter(d => d.message.includes('Type mismatch'));
      assert.ok(typeDiags.length > 0);
    });

    it('should report error when "to" operator has non-variable LHS', () => {
      const { ast } = parseDocument(':: Start\n<<set 5 to $x>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const opDiags = result.diagnostics.filter(d => d.message.includes("Operator 'to' requires a variable"));
      assert.ok(opDiags.length > 0);
    });

    it('should allow valid "to" assignments', () => {
      const { ast } = parseDocument(':: Start\n<<set $x to 5>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const opDiags = result.diagnostics.filter(d => d.message.includes("Operator 'to'"));
      assert.strictEqual(opDiags.length, 0);
    });
  });

  describe('Semantic tokens', () => {
    it('should emit semantic tokens for macros', () => {
      const { ast } = parseDocument(':: Start\n<<set $x to 5>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const macroTokens = result.semanticTokens.filter(t => t.tokenType === 'macro');
      assert.ok(macroTokens.length > 0);
      // Macro name 'set' starts after '<<' (position 11 in full doc with ':: Start\n')
      assert.ok(macroTokens[0]!.range.start > 0);
    });

    it('should emit semantic tokens for variables', () => {
      const { ast } = parseDocument(':: Start\n<<print $myVar>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const varTokens = result.semanticTokens.filter(t => t.tokenType === 'variable');
      assert.ok(varTokens.length > 0);
    });

    it('should emit semantic tokens for passages', () => {
      const { ast } = parseDocument(':: MyPassage\nContent');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const passageTokens = result.semanticTokens.filter(t => t.tokenType === 'passage');
      assert.ok(passageTokens.length > 0);
    });

    it('should emit semantic tokens for links', () => {
      const { ast } = parseDocument(':: Start\n[[Target]]');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const passageTokens = result.semanticTokens.filter(t => t.tokenType === 'passage');
      assert.ok(passageTokens.length > 0);
    });

    it('should emit semantic tokens for strings and numbers', () => {
      const { ast } = parseDocument(':: Start\n<<set $x to "hello">>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const stringTokens = result.semanticTokens.filter(t => t.tokenType === 'string');
      assert.ok(stringTokens.length > 0);
    });

    it('should emit semantic tokens for comments', () => {
      const { ast } = parseDocument(':: Start\n<!-- comment -->');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const commentTokens = result.semanticTokens.filter(t => t.tokenType === 'comment');
      assert.ok(commentTokens.length > 0);
    });
  });

  describe('Resolved links', () => {
    it('should mark local links as resolved', () => {
      const { ast } = parseDocument(':: Start\n[[Next]]\n\n:: Next\nContent');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      assert.ok(result.resolvedLinks.some(l => l.target === 'Next' && l.resolved));
    });

    it('should mark unknown links as unresolved', () => {
      const { ast } = parseDocument(':: Start\n[[Unknown]]');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      assert.ok(result.resolvedLinks.some(l => l.target === 'Unknown' && !l.resolved));
    });
  });

  describe('Nested macro analysis', () => {
    it('should analyze nested macros', () => {
      const { ast } = parseDocument(':: Start\n<<if true>><<print $x>><</if>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const macroTokens = result.semanticTokens.filter(t => t.tokenType === 'macro');
      assert.ok(macroTokens.length >= 2); // if and print
    });

    it('should validate macros in nested bodies', () => {
      const { ast } = parseDocument(':: Start\n<<if true>><<unknownNested>><</if>>');
      const result = analyzer.analyze(ast, 'test://test.tw', workspace);
      
      const macroDiags = result.diagnostics.filter(d => d.message.includes('Unknown macro'));
      assert.ok(macroDiags.length > 0);
    });
  });
});
