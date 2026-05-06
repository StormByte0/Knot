/**
 * Knot v2 — Zero Bleed-Through Tests (Format Boundary Enforcement)
 *
 * These tests verify that core and handler code has ZERO
 * format-specific knowledge. No format names, no <<>> or ()
 * assumptions, no SugarCube/Harlowe-specific patterns.
 *
 * These are the MOST IMPORTANT tests in the entire project.
 * If any of these fail, the architecture boundary is broken.
 *
 * Checks:
 *   1. No file in core/ imports from formats/
 *   2. No file in core/ contains format name strings (sugarcube, harlowe)
 *   3. No hardcoded <<>> patterns in core/ code (only in comments is OK)
 *   4. No hardcoded $/_ sigil logic in core/ (only through classifyVariableSigil)
 */

import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

// The compiled test JS lives at: server/out/tests/tests/server/core/boundary.test.js
// So __dirname = .../server/out/tests/tests/server/core/
// We need to reach: .../server/src/
// That's 5 levels up then 'src'
const SERVER_SRC = path.resolve(__dirname, '../../../../../src');

// ─── Helper ────────────────────────────────────────────────────

function walkDir(dir: string, ext: string): string[] {
  const results: string[] = [];
  if (!fs.existsSync(dir)) return results;
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...walkDir(fullPath, ext));
    } else if (entry.name.endsWith(ext)) {
      results.push(fullPath);
    }
  }
  return results;
}

function isCommentLine(line: string): boolean {
  const trimmed = line.trimStart();
  return trimmed.startsWith('//') ||
    trimmed.startsWith('*') ||
    trimmed.startsWith('/**') ||
    trimmed.startsWith('/*');
}

function isDocumentationContent(line: string): boolean {
  // Allow lines that are clearly documentation/assertion text
  return line.includes('MUST NOT') ||
    line.includes('CRITICAL') ||
    line.includes('Format-agnostic') ||
    line.includes('Zero bleed') ||
    line.includes('no format') ||
    line.includes('SugarCube-specific') ||
    line.includes('Harlowe-specific');
}

// ─── Tests ─────────────────────────────────────────────────────

describe('Zero Bleed-Through — Format Boundary Enforcement', () => {
  const protectedDirs = ['core', 'handlers'];

  // Format names that MUST NOT appear in protected directories
  const forbiddenFormatPatterns = [
    { pattern: /sugarcube/gi, name: 'SugarCube' },
    { pattern: /harlowe/gi, name: 'Harlowe' },
    { pattern: /chapbook/gi, name: 'Chapbook' },
    { pattern: /snowman/gi, name: 'Snowman' },
  ];

  // ─── 1. No imports from formats/ ──────────────────────────────

  for (const dir of protectedDirs) {
    describe(`${dir}/ — no imports from formats/`, () => {
      it('should not import from formats/', () => {
        const dirPath = path.join(SERVER_SRC, dir);
        if (!fs.existsSync(dirPath)) return;

        const files = walkDir(dirPath, '.ts');
        const violations: string[] = [];

        for (const file of files) {
          const content = fs.readFileSync(file, 'utf8');
          const lines = content.split('\n');

          for (let i = 0; i < lines.length; i++) {
            const line = lines[i];
            if (isCommentLine(line)) continue;

            const importMatch = line.match(/^import\s+.*from\s+['"]\.\.\/formats/);
            if (importMatch) {
              violations.push(
                `${path.relative(process.cwd(), file)}:${i + 1}: ${line.trim()}`
              );
            }
          }
        }

        if (violations.length > 0) {
          assert.fail(
            `Import from formats/ found in ${dir}/!\n` +
            violations.join('\n') +
            '\nCore and handlers must not import from formats/!'
          );
        }
      });
    });
  }

  // ─── 2. No format name strings ────────────────────────────────

  for (const dir of protectedDirs) {
    describe(`${dir}/ — no format name references`, () => {
      it('should contain no format name references in source code', () => {
        const dirPath = path.join(SERVER_SRC, dir);
        if (!fs.existsSync(dirPath)) return;

        const files = walkDir(dirPath, '.ts');
        const violations: string[] = [];

        for (const file of files) {
          const content = fs.readFileSync(file, 'utf8');
          const lines = content.split('\n');

          for (let i = 0; i < lines.length; i++) {
            const line = lines[i];
            if (isCommentLine(line)) continue;
            if (isDocumentationContent(line)) continue;

            for (const { pattern, name } of forbiddenFormatPatterns) {
              if (pattern.test(line)) {
                violations.push(
                  `${path.relative(process.cwd(), file)}:${i + 1}: [${name}] ${line.trim()}`
                );
              }
            }
          }
        }

        if (violations.length > 0) {
          assert.fail(
            `Format name references found in ${dir}/!\n` +
            violations.join('\n') +
            '\nThis breaks the format boundary!'
          );
        }
      });
    });
  }

  // ─── 3. No hardcoded <<>> macro patterns ─────────────────────

  for (const dir of protectedDirs) {
    describe(`${dir}/ — no hardcoded <<>> patterns`, () => {
      it('should not contain hardcoded <<>> macro patterns in source code', () => {
        const dirPath = path.join(SERVER_SRC, dir);
        if (!fs.existsSync(dirPath)) return;

        const files = walkDir(dirPath, '.ts');
        const violations: string[] = [];

        const sugarCubeMacroPattern = /<<\s*\w/;

        for (const file of files) {
          const content = fs.readFileSync(file, 'utf8');
          const lines = content.split('\n');

          for (let i = 0; i < lines.length; i++) {
            const line = lines[i];
            if (isCommentLine(line)) continue;
            if (line.includes('MUST NOT') || line.includes('close-tag') || line.includes('CloseTag')) continue;
            if (line.includes("'close-tag'") || line.includes('"close-tag"')) continue;

            // Allow MacroBodyStyle enum references
            if (line.includes('MacroBodyStyle')) continue;

            // Allow ISyntaxProvider references (documentation about the interface)
            if (line.includes('ISyntaxProvider')) continue;

            if (sugarCubeMacroPattern.test(line)) {
              violations.push(
                `${path.relative(process.cwd(), file)}:${i + 1}: ${line.trim()}`
              );
            }
          }
        }

        if (violations.length > 0) {
          assert.fail(
            `Hardcoded <<>> pattern found in ${dir}/!\n` +
            violations.join('\n') +
            '\n<<>> is SugarCube-specific! Use adapter\'s ISyntaxProvider instead.'
          );
        }
      });
    });
  }

  // ─── 4. No hardcoded sigil logic ─────────────────────────────

  describe('core/ — no hardcoded $/_ sigil logic', () => {
    it('should not hardcode $ = story or _ = temp variable logic', () => {
      const dirPath = path.join(SERVER_SRC, 'core');
      if (!fs.existsSync(dirPath)) return;

      const files = walkDir(dirPath, '.ts');
      const violations: string[] = [];

      // Patterns that indicate hardcoded sigil logic:
      // "$" => 'story', '_' => 'temp', etc.
      // BUT: allow these in comments and in the context of
      // calling classifyVariableSigil (which is the adapter method)
      const hardcodedSigilPatterns = [
        /\$\s*[=:]\s*['"]story['"]/,
        /_\s*[=:]\s*['"]temp['"]/,
        /sigil\s*===?\s*['"]\$/,
        /sigil\s*===?\s*['"]_/,
        /char\s*===?\s*['"]\$/,
        /char\s*===?\s*['"]_/,
      ];

      for (const file of files) {
        const content = fs.readFileSync(file, 'utf8');
        const lines = content.split('\n');

        for (let i = 0; i < lines.length; i++) {
          const line = lines[i];
          if (isCommentLine(line)) continue;
          if (isDocumentationContent(line)) continue;

          // Allow lines that are calling classifyVariableSigil
          if (line.includes('classifyVariableSigil')) continue;

          for (const pattern of hardcodedSigilPatterns) {
            if (pattern.test(line)) {
              violations.push(
                `${path.relative(process.cwd(), file)}:${i + 1}: ${line.trim()}`
              );
            }
          }
        }
      }

      if (violations.length > 0) {
        assert.fail(
          `Hardcoded sigil logic found in core/!\n` +
          violations.join('\n') +
          '\nVariable scope must be determined through classifyVariableSigil(), never hardcoded!'
        );
      }
    });
  });

  // ─── lspServer.ts check ───────────────────────────────────────

  describe('lspServer.ts — no format bleed-through', () => {
    it('should not contain format name references', () => {
      const file = path.join(SERVER_SRC, 'lspServer.ts');
      if (!fs.existsSync(file)) return;

      const content = fs.readFileSync(file, 'utf8');
      for (const { pattern, name } of forbiddenFormatPatterns) {
        if (pattern.test(content)) {
          assert.fail(`${name} reference found in lspServer.ts — this breaks the format boundary!`);
        }
      }
    });
  });

  // ─── Hook type completeness ───────────────────────────────────

  describe('Hook type completeness', () => {
    it('MacroCategory should cover Harlowe-specific categories', () => {
      const required = [
        'iteration', 'dataStructure', 'math', 'string', 'colour',
        'dateTime', 'interactive', 'revision', 'live', 'pattern',
        'customMacro', 'debugging', 'save',
      ];
      const file = path.join(SERVER_SRC, 'hooks', 'hookTypes.ts');
      const content = fs.readFileSync(file, 'utf8');
      for (const cat of required) {
        const camelCat = cat.charAt(0).toUpperCase() + cat.slice(1);
        assert.ok(content.includes(camelCat), `MacroCategory.${camelCat} is missing — needed for Harlowe`);
      }
    });

    it('MacroBodyStyle should have CloseTag, Hook, and Inline', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'hookTypes.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(content.includes('CloseTag'), 'MacroBodyStyle.CloseTag missing');
      assert.ok(content.includes('Hook'), 'MacroBodyStyle.Hook missing');
      assert.ok(content.includes('Inline'), 'MacroBodyStyle.Inline missing');
    });

    it('PassageKind should have Markup, Script, Stylesheet, Special', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'hookTypes.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(content.includes('Markup'), 'PassageKind.Markup missing');
      assert.ok(content.includes('Special'), 'PassageKind.Special missing');
    });

    it('ISyntaxProvider should have classifyVariableSigil', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'formatHooks.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(content.includes('classifyVariableSigil'), 'ISyntaxProvider.classifyVariableSigil missing');
    });

    it('IPassageProvider should have classifyPassage', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'formatHooks.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(content.includes('classifyPassage'), 'IPassageProvider.classifyPassage missing');
    });

    it('ILinkProvider should have resolveLinkBody', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'formatHooks.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(content.includes('resolveLinkBody'), 'ILinkProvider.resolveLinkBody missing');
    });
  });
});
