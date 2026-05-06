/**
 * Knot v2 — Hooks Re-exports
 *
 * Re-exports enums from hookTypes and types from formats/_types.
 * The old IFormatProvider/IMacroProvider/etc. interfaces have been
 * replaced by the FormatModule + capability bag pattern in _types.ts.
 *
 * Core files import from here for backward compatibility,
 * but should migrate to importing FormatModule directly
 * from formats/_types as they're updated.
 */

export {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageType,
  PassageKind,
  LinkKind,
  PassageRefKind,
} from './hookTypes';

// Re-export FormatModule types from the new location
export type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  BodyToken,
  LinkResolution,
  SpecialPassageDef,
  MacroDef,
  MacroSignatureDef,
  MacroArgDef,
  VariableSigilDef,
  MacroDelimiters,
  DiagnosticRuleDef,
  DiagnosticResult,
  DiagnosticCheckContext,
  MacroCapability,
  VariableCapability,
  CustomMacroCapability,
  DiagnosticCapability,
  SourceRange,
  PassageRef,
} from '../formats/_types';

export { CORE_DIAGNOSTIC_RULES } from '../formats/_types';

// Re-export registry for convenience
export { FormatRegistry } from '../formats/formatRegistry';
