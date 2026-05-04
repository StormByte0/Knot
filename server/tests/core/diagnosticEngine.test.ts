import { strict as assert } from 'assert';
import { DiagnosticEngine, DiagnosticRule, RuleDiagnostic } from '../../src/diagnosticEngine';
import { DiagnosticSeverity } from 'vscode-languageserver/node';

describe('DiagnosticEngine', () => {
  describe('Default configuration', () => {
    it('has all 9 rules enabled by default', () => {
      const engine = new DiagnosticEngine();
      const allRules = Object.values(DiagnosticRule);
      assert.strictEqual(allRules.length, 9);
      for (const rule of allRules) {
        assert.strictEqual(engine.isEnabled(rule), true, `Rule ${rule} should be enabled by default`);
      }
    });

    it('UnknownPassage defaults to Warning', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Warning);
    });

    it('UnknownMacro defaults to Warning', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownMacro), DiagnosticSeverity.Warning);
    });

    it('DuplicatePassage defaults to Error', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.DuplicatePassage), DiagnosticSeverity.Error);
    });

    it('TypeMismatch defaults to Error', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.TypeMismatch), DiagnosticSeverity.Error);
    });

    it('UnreachablePassage defaults to Warning', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnreachablePassage), DiagnosticSeverity.Warning);
    });

    it('ContainerStructure defaults to Error', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.ContainerStructure), DiagnosticSeverity.Error);
    });

    it('DeprecatedMacro defaults to Warning', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.DeprecatedMacro), DiagnosticSeverity.Warning);
    });

    it('MissingRequiredArg defaults to Error', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.MissingRequiredArg), DiagnosticSeverity.Error);
    });

    it('AssignmentTarget defaults to Error', () => {
      const engine = new DiagnosticEngine();
      assert.strictEqual(engine.getSeverity(DiagnosticRule.AssignmentTarget), DiagnosticSeverity.Error);
    });
  });

  describe('configure()', () => {
    it('can change severity', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.UnknownPassage, { severity: DiagnosticSeverity.Error });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Error);
    });

    it('can change enabled state', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.UnknownPassage, { enabled: false });
      assert.strictEqual(engine.isEnabled(DiagnosticRule.UnknownPassage), false);
    });

    it('partial update merges with existing config', () => {
      const engine = new DiagnosticEngine();
      // Default: UnknownPassage = Warning, enabled = true
      engine.configure(DiagnosticRule.UnknownPassage, { severity: DiagnosticSeverity.Error });
      // Only severity changed; enabled should still be true
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Error);
      assert.strictEqual(engine.isEnabled(DiagnosticRule.UnknownPassage), true);

      // Now disable — severity should remain Error
      engine.configure(DiagnosticRule.UnknownPassage, { enabled: false });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Error);
      assert.strictEqual(engine.isEnabled(DiagnosticRule.UnknownPassage), false);
    });
  });

  describe('isEnabled()', () => {
    it('returns false after disabling a rule', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.DuplicatePassage, { enabled: false });
      assert.strictEqual(engine.isEnabled(DiagnosticRule.DuplicatePassage), false);
    });

    it('returns true for unknown rules (fallback)', () => {
      const engine = new DiagnosticEngine();
      // isEnabled uses ?? true when the rule is not in the config map.
      // Casting a non-enum string to DiagnosticRule simulates an unknown rule.
      const unknown = 'nonexistent-rule' as DiagnosticRule;
      assert.strictEqual(engine.isEnabled(unknown), true);
    });
  });

  describe('getSeverity()', () => {
    it('returns default Warning for unknown rules', () => {
      const engine = new DiagnosticEngine();
      const unknown = 'nonexistent-rule' as DiagnosticRule;
      assert.strictEqual(engine.getSeverity(unknown), DiagnosticSeverity.Warning);
    });

    it('returns configured severity after configure()', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.TypeMismatch, { severity: DiagnosticSeverity.Warning });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.TypeMismatch), DiagnosticSeverity.Warning);
    });
  });

  describe('createDiagnostic()', () => {
    const range = { start: 0, end: 5 };

    it('returns RuleDiagnostic when rule is enabled', () => {
      const engine = new DiagnosticEngine();
      const result = engine.createDiagnostic(DiagnosticRule.UnknownPassage, 'test message', range);
      assert.ok(result !== null);
      assert.strictEqual(result!.rule, DiagnosticRule.UnknownPassage);
      assert.strictEqual(result!.message, 'test message');
      assert.deepStrictEqual(result!.range, range);
    });

    it('returns null when rule is disabled', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.UnknownPassage, { enabled: false });
      const result = engine.createDiagnostic(DiagnosticRule.UnknownPassage, 'test message', range);
      assert.strictEqual(result, null);
    });
  });

  describe('toParseDiagnostic()', () => {
    const range = { start: 10, end: 20 };

    it('converts Error severity to "error" string', () => {
      const engine = new DiagnosticEngine();
      // DuplicatePassage defaults to Error
      const ruleDiag: RuleDiagnostic = {
        rule: DiagnosticRule.DuplicatePassage,
        message: 'dup',
        range,
      };
      const parseDiag = engine.toParseDiagnostic(ruleDiag);
      assert.strictEqual(parseDiag.severity, 'error');
      assert.strictEqual(parseDiag.message, 'dup');
      assert.deepStrictEqual(parseDiag.range, range);
    });

    it('converts Warning severity to "warning" string', () => {
      const engine = new DiagnosticEngine();
      // UnknownPassage defaults to Warning
      const ruleDiag: RuleDiagnostic = {
        rule: DiagnosticRule.UnknownPassage,
        message: 'unknown',
        range,
      };
      const parseDiag = engine.toParseDiagnostic(ruleDiag);
      assert.strictEqual(parseDiag.severity, 'warning');
    });

    it('respects overridden severity', () => {
      const engine = new DiagnosticEngine();
      engine.configure(DiagnosticRule.UnknownPassage, { severity: DiagnosticSeverity.Error });
      const ruleDiag: RuleDiagnostic = {
        rule: DiagnosticRule.UnknownPassage,
        message: 'now error',
        range,
      };
      const parseDiag = engine.toParseDiagnostic(ruleDiag);
      assert.strictEqual(parseDiag.severity, 'error');
    });
  });

  describe('toParseDiagnostics()', () => {
    it('batch converts RuleDiagnostics to ParseDiagnostics', () => {
      const engine = new DiagnosticEngine();
      const range1 = { start: 0, end: 5 };
      const range2 = { start: 10, end: 15 };
      const ruleDiags: RuleDiagnostic[] = [
        { rule: DiagnosticRule.DuplicatePassage, message: 'dup', range: range1 },
        { rule: DiagnosticRule.UnknownPassage, message: 'unknown', range: range2 },
      ];
      const parseDiags = engine.toParseDiagnostics(ruleDiags);
      assert.strictEqual(parseDiags.length, 2);
      assert.strictEqual(parseDiags[0]!.severity, 'error');
      assert.strictEqual(parseDiags[0]!.message, 'dup');
      assert.strictEqual(parseDiags[1]!.severity, 'warning');
      assert.strictEqual(parseDiags[1]!.message, 'unknown');
    });
  });

  describe('configureFromLintConfig()', () => {
    it('maps legacy key unknownPassage to DiagnosticRule.UnknownPassage', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ unknownPassage: DiagnosticSeverity.Error });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Error);
    });

    it('maps legacy key unknownMacro to DiagnosticRule.UnknownMacro', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ unknownMacro: DiagnosticSeverity.Error });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownMacro), DiagnosticSeverity.Error);
    });

    it('maps legacy key duplicatePassage to DiagnosticRule.DuplicatePassage', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ duplicatePassage: DiagnosticSeverity.Warning });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.DuplicatePassage), DiagnosticSeverity.Warning);
    });

    it('maps legacy key typeMismatch to DiagnosticRule.TypeMismatch', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ typeMismatch: DiagnosticSeverity.Warning });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.TypeMismatch), DiagnosticSeverity.Warning);
    });

    it('maps legacy key unreachablePassage to DiagnosticRule.UnreachablePassage', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ unreachablePassage: DiagnosticSeverity.Error });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnreachablePassage), DiagnosticSeverity.Error);
    });

    it('maps legacy key containerStructure to DiagnosticRule.ContainerStructure', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({ containerStructure: DiagnosticSeverity.Warning });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.ContainerStructure), DiagnosticSeverity.Warning);
    });

    it('ignores unrecognized legacy keys', () => {
      const engine = new DiagnosticEngine();
      // Should not throw and should not affect any known rule
      engine.configureFromLintConfig({ bogusKey: DiagnosticSeverity.Error } as Record<string, DiagnosticSeverity>);
      // Verify defaults unchanged
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Warning);
    });

    it('maps multiple keys at once', () => {
      const engine = new DiagnosticEngine();
      engine.configureFromLintConfig({
        unknownPassage: DiagnosticSeverity.Error,
        unknownMacro: DiagnosticSeverity.Error,
        duplicatePassage: DiagnosticSeverity.Warning,
      });
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownPassage), DiagnosticSeverity.Error);
      assert.strictEqual(engine.getSeverity(DiagnosticRule.UnknownMacro), DiagnosticSeverity.Error);
      assert.strictEqual(engine.getSeverity(DiagnosticRule.DuplicatePassage), DiagnosticSeverity.Warning);
    });
  });

  describe('toLintConfig()', () => {
    it('round-trips with configureFromLintConfig', () => {
      const engine1 = new DiagnosticEngine();
      engine1.configureFromLintConfig({
        unknownPassage: DiagnosticSeverity.Error,
        unknownMacro: DiagnosticSeverity.Warning,
        duplicatePassage: DiagnosticSeverity.Warning,
        typeMismatch: DiagnosticSeverity.Error,
        unreachablePassage: DiagnosticSeverity.Error,
        containerStructure: DiagnosticSeverity.Warning,
      });
      const config = engine1.toLintConfig();

      // Apply the extracted config to a fresh engine
      const engine2 = new DiagnosticEngine();
      engine2.configureFromLintConfig(config);

      // Both engines should now report identical severities for all 6 mapped rules
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.UnknownPassage), engine1.getSeverity(DiagnosticRule.UnknownPassage));
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.UnknownMacro), engine1.getSeverity(DiagnosticRule.UnknownMacro));
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.DuplicatePassage), engine1.getSeverity(DiagnosticRule.DuplicatePassage));
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.TypeMismatch), engine1.getSeverity(DiagnosticRule.TypeMismatch));
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.UnreachablePassage), engine1.getSeverity(DiagnosticRule.UnreachablePassage));
      assert.strictEqual(engine2.getSeverity(DiagnosticRule.ContainerStructure), engine1.getSeverity(DiagnosticRule.ContainerStructure));
    });

    it('returns default severities when no overrides applied', () => {
      const engine = new DiagnosticEngine();
      const config = engine.toLintConfig();
      assert.strictEqual(config.unknownPassage, DiagnosticSeverity.Warning);
      assert.strictEqual(config.unknownMacro, DiagnosticSeverity.Warning);
      assert.strictEqual(config.duplicatePassage, DiagnosticSeverity.Error);
      assert.strictEqual(config.typeMismatch, DiagnosticSeverity.Error);
      assert.strictEqual(config.unreachablePassage, DiagnosticSeverity.Warning);
      assert.strictEqual(config.containerStructure, DiagnosticSeverity.Error);
    });
  });
});
