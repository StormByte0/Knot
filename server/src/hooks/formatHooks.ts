/**
 * Knot v2 — Format Hook Interfaces (DEPRECATED)
 *
 * This module has been replaced by the FormatModule type system in formats/_types.ts.
 * The IFormatProvider/IMacroProvider/etc. interfaces have been superseded by the
 * FormatModule + capability bag architecture.
 *
 * All consumers should import types from '../formats/_types' or '../hooks/index'.
 *
 * This file is kept only to avoid breaking any remaining import paths.
 * It will be removed in a future cleanup pass.
 */

// Re-export enum types that are still in use
export {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageType,
  PassageKind,
  LinkKind,
  PassageRefKind,
} from './hookTypes';

// Re-export types from the new system
export type {
  FormatModule as IFormatProvider,
  MacroDef as MacroDefinition,
  MacroSignatureDef as MacroSignature,
  MacroArgDef as MacroArg,
  DiagnosticRuleDef as FormatDiagnosticRule,
  DiagnosticResult,
  LinkResolution as ParsedLink,
  BodyToken as AdapterToken,
  FormatASTNodeTypes as FormatASTNodeType,
  PassageRef,
} from '../formats/_types';

export { CORE_DIAGNOSTIC_RULES } from '../formats/_types';

// FormatAdapterExport is replaced by FormatModule
export type { FormatModule as FormatAdapterExport } from '../formats/_types';
