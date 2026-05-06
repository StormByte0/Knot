/**
 * Knot v2 — Test Fixtures
 *
 * Shared mock providers for testing core/handler code.
 * CRITICAL: Tests for core/handlers MUST use these mocks,
 * never real format adapters. This enforces the boundary.
 */

import {
  IFormatProvider,
  IMacroProvider,
  IPassageProvider,
  IDiagnosticProvider,
  ILinkProvider,
  ISyntaxProvider,
  MacroDefinition,
  PassageTypeDefinition,
  DiagnosticResult,
  ParsedLink,
  AdapterToken,
} from '../../server/src/hooks/formatHooks';

import {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageType,
  PassageKind,
  LinkKind,
  DiagnosticRule,
  FormatCapability,
} from '../../server/src/hooks/hookTypes';

export class MockMacroProvider implements IMacroProvider {
  private macros: Map<string, MacroDefinition> = new Map();
  addMacro(def: MacroDefinition): void {
    this.macros.set(def.name, def);
    if (def.aliases) for (const alias of def.aliases) this.macros.set(alias, def);
  }
  getMacros(): MacroDefinition[] { return Array.from(new Set(this.macros.values())); }
  getMacroByName(name: string): MacroDefinition | undefined { return this.macros.get(name); }
  isMacroKnown(name: string): boolean { return this.macros.has(name); }
}

export class MockPassageProvider implements IPassageProvider {
  private passageTypes: Map<PassageType, PassageTypeDefinition> = new Map();
  private specialTags: string[] = [];
  private storyDataName = 'StoryData';
  private startName = 'Start';
  private classifyFn: ((name: string, tags: string[]) => PassageKind | null) | null = null;
  configure(opts: any): void { Object.assign(this, opts); }
  getPassageTypes(): Map<PassageType, PassageTypeDefinition> { return this.passageTypes; }
  getSpecialTags(): string[] { return this.specialTags; }
  getStoryDataPassageName(): string { return this.storyDataName; }
  getStartPassageName(): string { return this.startName; }
  getPassageHeaderPattern(): RegExp { return /^::\s*([^\[\]]+)(?:\s*\[([^\]]*)\])?\s*$/m; }
  getSpecialPassagePattern(_type: PassageType): RegExp | undefined { return undefined; }
  classifyPassage(name: string, tags: string[]): PassageKind | null {
    return this.classifyFn ? this.classifyFn(name, tags) : null;
  }
}

export class MockDiagnosticProvider implements IDiagnosticProvider {
  private supportedRules: DiagnosticRule[] = [];
  addRule(rule: DiagnosticRule, severity: string): void { this.supportedRules.push(rule); }
  getSupportedRules(): DiagnosticRule[] { return this.supportedRules; }
  getRuleSeverity(_rule: DiagnosticRule): string | undefined { return undefined; }
  checkMacroUsage(_name: string, _args: any[]): DiagnosticResult[] { return []; }
  checkPassageStructure(_pt: PassageType, _c: string): DiagnosticResult[] { return []; }
}

export class MockLinkProvider implements ILinkProvider {
  getLinkKinds(): LinkKind[] { return [LinkKind.Passage]; }
  resolveLinkBody(rawBody: string): ParsedLink | undefined {
    const ra = rawBody.lastIndexOf('->');
    if (ra >= 0) return { kind: LinkKind.Passage, target: rawBody.substring(ra + 2).trim(), displayText: rawBody.substring(0, ra).trim() };
    return { kind: LinkKind.Passage, target: rawBody.trim() };
  }
  parseLinkSyntax(text: string): ParsedLink | undefined {
    const m = text.match(/^\[\[(.+?)\]\]$/);
    return m ? this.resolveLinkBody(m[1]) : undefined;
  }
  resolveLinkTarget(link: ParsedLink): string | undefined { return link.kind === LinkKind.Passage ? link.target : undefined; }
}

export class MockSyntaxProvider implements ISyntaxProvider {
  private bodyStyle: MacroBodyStyle = MacroBodyStyle.Inline;
  private macroPat: RegExp | null = null;
  private varPat: RegExp | null = null;
  configure(opts: any): void { Object.assign(this, opts); }
  getMacroBodyStyle(): MacroBodyStyle { return this.bodyStyle; }
  lexBody(_body: string): AdapterToken[] { return []; }
  getMacroPattern(): RegExp | null { return this.macroPat; }
  getVariablePattern(): RegExp | null { return this.varPat; }
  getMacroTriggerChars(): string[] { return []; }
  getVariableTriggerChars(): string[] { return []; }
  getMacroCallPrefix(): string { return ''; }
  getMacroCallSuffix(): string { return ''; }
  getMacroClosePrefix(): string { return ''; }
  getMacroCloseSuffix(): string { return ''; }
  classifyVariableSigil(_sigil: string): 'story' | 'temp' | null { return null; }
}

export class MockFormatProvider implements IFormatProvider {
  readonly formatId = 'mock';
  readonly formatName = 'Mock Format';
  readonly formatVersion = '0.0.1';
  readonly capabilities = new Set<FormatCapability>();
  readonly macroProvider = new MockMacroProvider();
  readonly passageProvider = new MockPassageProvider();
  readonly diagnosticProvider = new MockDiagnosticProvider();
  readonly linkProvider = new MockLinkProvider();
  readonly syntaxProvider = new MockSyntaxProvider();
  getMacroProvider(): IMacroProvider { return this.macroProvider; }
  getPassageProvider(): IPassageProvider { return this.passageProvider; }
  getDiagnosticProvider(): IDiagnosticProvider { return this.diagnosticProvider; }
  getLinkProvider(): ILinkProvider { return this.linkProvider; }
  getSyntaxProvider(): ISyntaxProvider { return this.syntaxProvider; }
}

export function createSampleMacro(overrides?: Partial<MacroDefinition>): MacroDefinition {
  return { name: 'testMacro', category: MacroCategory.Output, kind: MacroKind.Command, description: 'Test', signatures: [{ args: [{ name: 'arg1', type: 'string', required: true }] }], ...overrides };
}
