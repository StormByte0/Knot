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
 *   4. No hardcoded $/_ sigil logic in core/ (only through VariableCapability)
 *   5. Enum completeness checks for the slimmed enums
 */

import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

import {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageType,
  PassageKind,
  LinkKind,
  PassageRefKind,
} from '../../../server/src/hooks/hookTypes';

// The compiled test JS lives at: .../server/out/tests/tests/server/core/boundary.test.js
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
              // Allow imports from formats/_types, formats/formatRegistry, formats/index (the approved boundary)
              // Forbid imports from formats/sugarcube/, formats/harlowe/, etc.
              const isAllowed = line.includes('formats/_types') || line.includes('formats/formatRegistry') || line.includes('formats/index');
              if (!isAllowed) {
                violations.push(
                  `${path.relative(process.cwd(), file)}:${i + 1}: ${line.trim()}`
                );
              }
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
            '\n<<>> is SugarCube-specific! Use FormatModule\'s macroDelimiters instead.'
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
      // using VariableCapability from the format module
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

          // Allow lines that reference VariableCapability or sigils from format modules
          if (line.includes('VariableCapability') || line.includes('variables')) continue;

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
          '\nVariable scope must be determined through VariableCapability, never hardcoded!'
        );
      }
    });
  });

  // ─── 5. lspServer.ts check ───────────────────────────────────

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

  // ─── 6. Hook type completeness (slimmed enums) ───────────────

  describe('Hook type completeness', () => {
    it('MacroCategory should have exactly 8 values', () => {
      const values = Object.values(MacroCategory);
      assert.strictEqual(values.length, 8, `MacroCategory should have 8 values, got ${values.length}`);
    });

    it('MacroCategory should have required universal categories', () => {
      const required = ['navigation', 'output', 'control', 'variable', 'styling', 'system', 'utility', 'custom'];
      const values = Object.values(MacroCategory);
      for (const cat of required) {
        assert.ok(values.includes(cat as MacroCategory), `MacroCategory missing: ${cat}`);
      }
    });

    it('MacroKind should have exactly 3 values (Changer, Command, Instant)', () => {
      const values = Object.values(MacroKind);
      assert.strictEqual(values.length, 3, `MacroKind should have 3 values, got ${values.length}`);
      assert.ok(values.includes('changer' as MacroKind), 'Missing Changer');
      assert.ok(values.includes('command' as MacroKind), 'Missing Command');
      assert.ok(values.includes('instant' as MacroKind), 'Missing Instant');
    });

    it('MacroBodyStyle should have exactly 3 values (CloseTag, Hook, Inline)', () => {
      const values = Object.values(MacroBodyStyle);
      assert.strictEqual(values.length, 3, `MacroBodyStyle should have 3 values, got ${values.length}`);
      assert.ok(values.includes('close-tag' as MacroBodyStyle), 'Missing CloseTag');
      assert.ok(values.includes('hook' as MacroBodyStyle), 'Missing Hook');
      assert.ok(values.includes('inline' as MacroBodyStyle), 'Missing Inline');
    });

    it('PassageType should have exactly 6 values', () => {
      const values = Object.values(PassageType);
      assert.strictEqual(values.length, 6, `PassageType should have 6 values, got ${values.length}`);
    });

    it('PassageKind should have exactly 4 values (Markup, Script, Stylesheet, Special)', () => {
      const values = Object.values(PassageKind);
      assert.strictEqual(values.length, 4, `PassageKind should have 4 values, got ${values.length}`);
      assert.ok(values.includes('markup' as PassageKind), 'Missing Markup');
      assert.ok(values.includes('script' as PassageKind), 'Missing Script');
      assert.ok(values.includes('stylesheet' as PassageKind), 'Missing Stylesheet');
      assert.ok(values.includes('special' as PassageKind), 'Missing Special');
    });

    it('LinkKind should have exactly 3 values (Passage, External, Custom)', () => {
      const values = Object.values(LinkKind);
      assert.strictEqual(values.length, 3, `LinkKind should have 3 values, got ${values.length}`);
      assert.ok(values.includes('passage' as LinkKind), 'Missing Passage');
      assert.ok(values.includes('external' as LinkKind), 'Missing External');
      assert.ok(values.includes('custom' as LinkKind), 'Missing Custom');
    });

    it('PassageRefKind should have exactly 4 values (Link, Macro, API, Implicit)', () => {
      const values = Object.values(PassageRefKind);
      assert.strictEqual(values.length, 4, `PassageRefKind should have 4 values, got ${values.length}`);
      assert.ok(values.includes('link' as PassageRefKind), 'Missing Link');
      assert.ok(values.includes('macro' as PassageRefKind), 'Missing Macro');
      assert.ok(values.includes('api' as PassageRefKind), 'Missing API');
      assert.ok(values.includes('implicit' as PassageRefKind), 'Missing Implicit');
    });

    it('NO FormatCapability enum should exist in hookTypes', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'hookTypes.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(!content.includes('FormatCapability'), 'FormatCapability enum should NOT exist — capabilities are now checked via bag presence on FormatModule');
    });

    it('NO DiagnosticRule enum should exist in hookTypes', () => {
      const file = path.join(SERVER_SRC, 'hooks', 'hookTypes.ts');
      const content = fs.readFileSync(file, 'utf8');
      assert.ok(!content.includes('DiagnosticRule'), 'DiagnosticRule enum should NOT exist — rules now use string IDs via DiagnosticRuleDef');
    });
  });
});
