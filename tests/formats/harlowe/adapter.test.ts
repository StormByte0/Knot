/**
 * Knot v2 — Harlowe Adapter Tests
 *
 * Exhaustive tests for the Harlowe 3 format adapter.
 * Verifies every provider interface is correctly implemented.
 */

import * as assert from 'assert';

// Import the adapter
import { HarloweAdapter } from '../../../server/src/formats/harlowe/adapter';
import { MacroCategory, MacroKind, MacroBodyStyle, PassageType, PassageKind, LinkKind, DiagnosticRule } from '../../../server/src/hooks/hookTypes';

describe('HarloweAdapter', () => {
  let adapter: HarloweAdapter;

  before(() => {
    adapter = new HarloweAdapter();
  });

  // ─── IFormatProvider ──────────────────────────────────────────

  describe('IFormatProvider', () => {
    it('should have correct formatId', () => {
      assert.strictEqual(adapter.formatId, 'harlowe-3');
    });

    it('should have correct formatName', () => {
      assert.strictEqual(adapter.formatName, 'Harlowe');
    });

    it('should have version 3.3.8', () => {
      assert.strictEqual(adapter.formatVersion, '3.3.8');
    });

    it('should support all expected capabilities', () => {
      const { FormatCapability } = require('../../../server/src/hooks/hookTypes');
      assert.ok(adapter.capabilities.has(FormatCapability.MacroCompletion), 'Missing MacroCompletion');
      assert.ok(adapter.capabilities.has(FormatCapability.PassageCompletion), 'Missing PassageCompletion');
      assert.ok(adapter.capabilities.has(FormatCapability.VariableCompletion), 'Missing VariableCompletion');
      assert.ok(adapter.capabilities.has(FormatCapability.MacroHover), 'Missing MacroHover');
      assert.ok(adapter.capabilities.has(FormatCapability.CustomDiagnostics), 'Missing CustomDiagnostics');
      assert.ok(adapter.capabilities.has(FormatCapability.BodyLexing), 'Missing BodyLexing');
    });

    it('should implement all 6 sub-providers', () => {
      assert.ok(adapter.getMacroProvider(), 'Missing macro provider');
      assert.ok(adapter.getPassageProvider(), 'Missing passage provider');
      assert.ok(adapter.getDiagnosticProvider(), 'Missing diagnostic provider');
      assert.ok(adapter.getLinkProvider(), 'Missing link provider');
      assert.ok(adapter.getSyntaxProvider(), 'Missing syntax provider');
    });
  });

  // ─── IMacroProvider ───────────────────────────────────────────

  describe('IMacroProvider', () => {
    const macroProvider = () => adapter.getMacroProvider();

    it('should have a non-empty macro catalog', () => {
      const macros = macroProvider().getMacros();
      assert.ok(macros.length > 100, `Expected 100+ macros, got ${macros.length}`);
    });

    it('should include all basic macros', () => {
      const basics = ['set:', 'put:', 'move:', 'print:', 'display:'];
      for (const name of basics) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing basic macro: ${name}`);
      }
    });

    it('should include all branching macros', () => {
      const branching = ['if:', 'unless:', 'else-if:', 'else:'];
      for (const name of branching) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing branching macro: ${name}`);
      }
    });

    it('should include all navigation macros', () => {
      const nav = ['go-to:', 'redirect:', 'undo:', 'restart:'];
      for (const name of nav) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing nav macro: ${name}`);
      }
    });

    it('should include link macros', () => {
      const links = ['link-goto:', 'link:', 'link-reveal:', 'link-repeat:', 'link-undo:', 'link-storylet:'];
      for (const name of links) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing link macro: ${name}`);
      }
    });

    it('should include math macros', () => {
      const math = ['abs:', 'cos:', 'sin:', 'sqrt:', 'random:', 'min:', 'max:'];
      for (const name of math) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing math macro: ${name}`);
      }
    });

    it('should include string macros', () => {
      const str = ['str:', 'lowercase:', 'uppercase:', 'split:', 'joined:'];
      for (const name of str) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing string macro: ${name}`);
      }
    });

    it('should include data structure macros', () => {
      const data = ['a:', 'dm:', 'ds:', 'range:', 'shuffled:'];
      for (const name of data) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing data macro: ${name}`);
      }
    });

    it('should include styling macros', () => {
      const styling = ['align:', 'bg:', 'font:', 'text-colour:', 'text-size:', 'css:'];
      for (const name of styling) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing styling macro: ${name}`);
      }
    });

    it('should include transition macros', () => {
      const t8n = ['transition:', 'transition-time:', 'transition-depart:', 'animate:'];
      for (const name of t8n) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing transition macro: ${name}`);
      }
    });

    it('should include mouse event macros', () => {
      const mouse = ['mouseover:', 'mouseout:', 'mouseover-replace:', 'mouseout-append:'];
      for (const name of mouse) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing mouse macro: ${name}`);
      }
    });

    it('should include click macros', () => {
      const click = ['click:', 'click-replace:', 'click-goto:', 'click-append:'];
      for (const name of click) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing click macro: ${name}`);
      }
    });

    it('should include live/timed macros', () => {
      const live = ['live:', 'stop:', 'event:', 'after:', 'more:'];
      for (const name of live) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing live macro: ${name}`);
      }
    });

    it('should include storylet macros', () => {
      const storylet = ['storylet:', 'open-storylets:', 'exclusivity:', 'urgency:'];
      for (const name of storylet) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing storylet macro: ${name}`);
      }
    });

    it('should include custom macro macros', () => {
      const custom = ['macro:', 'output:', 'output-data:', 'datatype:', 'partial:'];
      for (const name of custom) {
        assert.ok(macroProvider().isMacroKnown(name), `Missing custom macro macro: ${name}`);
      }
    });

    it('should resolve aliases', () => {
      // v6m: is alias for verbatim:
      assert.ok(macroProvider().isMacroKnown('v6m:'), 'v6m: alias not found');
      // b4r: is alias for border:
      assert.ok(macroProvider().isMacroKnown('b4r:'), 'b4r: alias not found');
      // a: is alias for array:
      assert.ok(macroProvider().isMacroKnown('a:'), 'a: alias not found');
      // dm: is alias for datamap:
      assert.ok(macroProvider().isMacroKnown('dm:'), 'dm: alias not found');
    });

    it('should classify changers correctly', () => {
      const changers = ['if:', 'unless:', 'else-if:', 'else:', 'for:', 'link:', 'link-reveal:', 'hidden:', 'live:'];
      for (const name of changers) {
        const macro = macroProvider().getMacroByName(name);
        assert.ok(macro, `Macro ${name} not found`);
        assert.strictEqual(macro!.kind, MacroKind.Changer, `${name} should be a Changer, got ${macro!.kind}`);
      }
    });

    it('should classify commands correctly', () => {
      const commands = ['go-to:', 'print:', 'link-goto:', 'link-undo:'];
      for (const name of commands) {
        const macro = macroProvider().getMacroByName(name);
        assert.ok(macro, `Macro ${name} not found`);
        assert.strictEqual(macro!.kind, MacroKind.Command, `${name} should be a Command, got ${macro!.kind}`);
      }
    });

    it('should classify instants correctly', () => {
      const instants = ['set:', 'put:', 'move:'];
      for (const name of instants) {
        const macro = macroProvider().getMacroByName(name);
        assert.ok(macro, `Macro ${name} not found`);
        assert.strictEqual(macro!.kind, MacroKind.Instant, `${name} should be an Instant, got ${macro!.kind}`);
      }
    });

    it('should have parent/child relationships for branching macros', () => {
      const ifMacro = macroProvider().getMacroByName('if:');
      assert.ok(ifMacro, 'if: macro not found');
      assert.ok(ifMacro!.children && ifMacro!.children.length > 0, 'if: should have children (else-if:, else:)');

      const elseIfMacro = macroProvider().getMacroByName('else-if:');
      assert.ok(elseIfMacro, 'else-if: macro not found');
      assert.ok(elseIfMacro!.parents && elseIfMacro!.parents.length > 0, 'else-if: should have parents (if:)');
    });

    it('should return undefined for unknown macros', () => {
      assert.strictEqual(macroProvider().getMacroByName('nonexistent:'), undefined);
      assert.strictEqual(macroProvider().isMacroKnown('nonexistent:'), false);
    });

    it('should have descriptions for all macros', () => {
      const macros = macroProvider().getMacros();
      for (const macro of macros) {
        assert.ok(macro.description.length > 0, `${macro.name} has no description`);
      }
    });

    it('should have at least one signature for each macro', () => {
      const macros = macroProvider().getMacros();
      for (const macro of macros) {
        assert.ok(macro.signatures.length > 0, `${macro.name} has no signatures`);
      }
    });
  });

  // ─── IPassageProvider ─────────────────────────────────────────

  describe('IPassageProvider', () => {
    const passageProvider = () => adapter.getPassageProvider();

    it('should recognize Harlowe special tags', () => {
      const tags = passageProvider().getSpecialTags();
      assert.ok(tags.includes('header'), 'Missing header tag');
      assert.ok(tags.includes('footer'), 'Missing footer tag');
      assert.ok(tags.includes('startup'), 'Missing startup tag');
      assert.ok(tags.includes('debug-header'), 'Missing debug-header tag');
      assert.ok(tags.includes('debug-footer'), 'Missing debug-footer tag');
      assert.ok(tags.includes('debug-startup'), 'Missing debug-startup tag');
    });

    it('should classify header passages as Special', () => {
      const kind = passageProvider().classifyPassage('MyHeader', ['header']);
      assert.strictEqual(kind, PassageKind.Special);
    });

    it('should classify startup passages as Special', () => {
      const kind = passageProvider().classifyPassage('MyStartup', ['startup']);
      assert.strictEqual(kind, PassageKind.Special);
    });

    it('should classify footer passages as Special', () => {
      const kind = passageProvider().classifyPassage('MyFooter', ['footer']);
      assert.strictEqual(kind, PassageKind.Special);
    });

    it('should return null for normal passages', () => {
      const kind = passageProvider().classifyPassage('MyPassage', []);
      assert.strictEqual(kind, null);
    });

    it('should include Harlowe passage types', () => {
      const types = passageProvider().getPassageTypes();
      assert.ok(types.has(PassageType.Header), 'Missing Header passage type');
      assert.ok(types.has(PassageType.Footer), 'Missing Footer passage type');
      assert.ok(types.has(PassageType.Startup), 'Missing Startup passage type');
    });

    it('should use StoryData as metadata passage', () => {
      assert.strictEqual(passageProvider().getStoryDataPassageName(), 'StoryData');
    });

    it('should use Start as start passage', () => {
      assert.strictEqual(passageProvider().getStartPassageName(), 'Start');
    });
  });

  // ─── ILinkProvider ────────────────────────────────────────────

  describe('ILinkProvider', () => {
    const linkProvider = () => adapter.getLinkProvider();

    it('should parse simple [[Target]] links', () => {
      const link = linkProvider().resolveLinkBody('Target');
      assert.ok(link);
      assert.strictEqual(link!.target, 'Target');
      assert.strictEqual(link!.kind, LinkKind.Passage);
    });

    it('should parse [[Text->Target]] with right arrow', () => {
      const link = linkProvider().resolveLinkBody('Click here->Next Room');
      assert.ok(link);
      assert.strictEqual(link!.target, 'Next Room');
      assert.strictEqual(link!.displayText, 'Click here');
    });

    it('should use rightmost -> as separator', () => {
      // [[A->B->C]] → text="A->B", target="C"
      const link = linkProvider().resolveLinkBody('A->B->C');
      assert.ok(link);
      assert.strictEqual(link!.target, 'C');
      assert.strictEqual(link!.displayText, 'A->B');
    });

    it('should parse [[Target<-Text]] with left arrow', () => {
      const link = linkProvider().resolveLinkBody('Next Room<-Click here');
      assert.ok(link);
      assert.strictEqual(link!.target, 'Next Room');
      assert.strictEqual(link!.displayText, 'Click here');
    });

    it('should use leftmost <- as separator', () => {
      // [[A<-B<-C]] → target="A", text="B<-C"
      const link = linkProvider().resolveLinkBody('A<-B<-C');
      assert.ok(link);
      assert.strictEqual(link!.target, 'A');
      assert.strictEqual(link!.displayText, 'B<-C');
    });

    it('should detect external URLs', () => {
      const link = linkProvider().resolveLinkBody('https://example.com');
      assert.ok(link);
      assert.strictEqual(link!.kind, LinkKind.External);
    });

    it('should NOT use pipe | as a link separator (Harlowe difference from SugarCube)', () => {
      // In Harlowe, | is for hook nametags, not link separators
      const link = linkProvider().resolveLinkBody('Text|Target');
      // This should NOT split on | — it should treat the whole thing as target
      assert.ok(link);
      // The whole string is the target since | is not a separator
      assert.ok(link!.target.includes('|'), 'Pipe should not be treated as separator in Harlowe');
    });

    it('should support Passage and External link kinds', () => {
      const kinds = linkProvider().getLinkKinds();
      assert.ok(kinds.includes(LinkKind.Passage), 'Missing Passage link kind');
      assert.ok(kinds.includes(LinkKind.External), 'Missing External link kind');
    });
  });

  // ─── ISyntaxProvider ──────────────────────────────────────────

  describe('ISyntaxProvider', () => {
    const syntaxProvider = () => adapter.getSyntaxProvider();

    it('should use Hook macro body style', () => {
      assert.strictEqual(syntaxProvider().getMacroBodyStyle(), MacroBodyStyle.Hook);
    });

    it('should provide a macro pattern for (name:) syntax', () => {
      const pattern = syntaxProvider().getMacroPattern();
      assert.ok(pattern, 'Macro pattern should not be null');

      // Should match (set: $x to 5)
      const testStr = '(set: $x to 5)';
      pattern.lastIndex = 0;
      const match = pattern.exec(testStr);
      assert.ok(match, 'Macro pattern should match (set: $x to 5)');
      assert.ok(match![1], 'Macro pattern should capture name');
    });

    it('should provide a variable pattern for $ and _', () => {
      const pattern = syntaxProvider().getVariablePattern();
      assert.ok(pattern, 'Variable pattern should not be null');

      // Should match $storyVar
      pattern.lastIndex = 0;
      const match1 = pattern.exec('$storyVar');
      assert.ok(match1, 'Variable pattern should match $storyVar');

      // Should match _tempVar
      pattern.lastIndex = 0;
      const match2 = pattern.exec('_tempVar');
      assert.ok(match2, 'Variable pattern should match _tempVar');
    });

    it('should lex passage body into adapter tokens', () => {
      const tokens = syntaxProvider().lexBody('(set: $x to 5)\nSome text\n(if: $x > 3)[Shown]');
      assert.ok(tokens.length > 0, 'Body lexing should produce tokens');

      // Should have at least a macro call token
      const macroTokens = tokens.filter(t => t.type === 'MacroCall');
      assert.ok(macroTokens.length >= 2, `Expected at least 2 MacroCall tokens, got ${macroTokens.length}`);
    });

    it('should identify hook open/close in body', () => {
      const tokens = syntaxProvider().lexBody('(if: $x)[hook text]');
      const hookTokens = tokens.filter(t => t.type === 'HookOpen' || t.type === 'HookClose');
      assert.ok(hookTokens.length >= 2, `Expected HookOpen and HookClose tokens, got ${hookTokens.length}`);
    });
  });

  // ─── IDiagnosticProvider ──────────────────────────────────────

  describe('IDiagnosticProvider', () => {
    const diagProvider = () => adapter.getDiagnosticProvider();

    it('should support Harlowe-specific diagnostic rules', () => {
      const rules = diagProvider().getSupportedRules();
      assert.ok(rules.includes(DiagnosticRule.UnknownMacro), 'Missing UnknownMacro rule');
      assert.ok(rules.includes(DiagnosticRule.InvalidHookStructure), 'Missing InvalidHookStructure rule');
      assert.ok(rules.includes(DiagnosticRule.InvalidChangerBinding), 'Missing InvalidChangerBinding rule');
    });
  });
});
