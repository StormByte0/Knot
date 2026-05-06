/**
 * Knot v2 — Chapbook 2 Insert Helpers
 *
 * Shared helper functions and type re-exports used by all
 * `inserts-*.ts` category files. Each category file imports
 * `insert`, `arg`, `sig`, and enums from here.
 *
 * Chapbook-specific: `arg` includes `embeddedLanguage` support
 * for marking arguments that contain JS/CSS/HTML.
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  MacroDef,
  MacroSignatureDef,
  MacroArgDef,
} from '../_types';

import {
  MacroCategory,
  MacroKind,
} from '../../hooks/hookTypes';

// Re-export so category files don't need a second import.
export { MacroCategory, MacroKind };

// ─── insert: primary insert builder ────────────────────────────

/**
 * Build an insert definition (Chapbook's equivalent of a macro def).
 * Inserts use {name} syntax instead of <<name>> or (name:).
 */
export function insert(
  name: string,
  category: MacroCategory,
  kind: MacroKind,
  description: string,
  signatures: MacroSignatureDef[],
  opts?: {
    aliases?: string[];
    deprecated?: boolean;
    deprecationMessage?: string;
    children?: string[];
    parents?: string[];
    categoryDetail?: string;
    hasBody?: boolean;
    isNavigation?: boolean;
    isInclude?: boolean;
    isConditional?: boolean;
    isAssignment?: boolean;
    passageArgPosition?: number;
  },
): MacroDef {
  return {
    name,
    aliases: opts?.aliases,
    category,
    categoryDetail: opts?.categoryDetail,
    kind,
    description,
    signatures,
    deprecated: opts?.deprecated,
    deprecationMessage: opts?.deprecationMessage,
    children: opts?.children,
    parents: opts?.parents,
    hasBody: opts?.hasBody,
    isNavigation: opts?.isNavigation,
    isInclude: opts?.isInclude,
    isConditional: opts?.isConditional,
    isAssignment: opts?.isAssignment,
    passageArgPosition: opts?.passageArgPosition,
  };
}

// ─── sig: signature builder ─────────────────────────────────────

/** Shorthand for a single signature. */
export function sig(args: MacroArgDef[], returnType?: string, description?: string): MacroSignatureDef {
  return { args, returnType, description };
}

// ─── arg: argument builder ──────────────────────────────────────

/**
 * Build a macro argument definition.
 * Chapbook extends the base arg with `embeddedLanguage` to mark
 * arguments that contain JS/CSS/HTML code (e.g. {if expression},
 * {debug expression}).
 */
export function arg(
  name: string,
  type: string,
  required: boolean,
  opts?: Partial<Pick<MacroArgDef, 'variadic' | 'description' | 'embeddedLanguage'>>,
): MacroArgDef {
  return {
    name,
    type,
    required,
    variadic: opts?.variadic,
    description: opts?.description,
    embeddedLanguage: opts?.embeddedLanguage,
  };
}
