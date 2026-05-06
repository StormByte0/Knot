/**
 * Knot v2 — Harlowe 3 Macro Helpers
 *
 * Shared helper functions and type re-exports used by all
 * `macros-*.ts` category files. Each category file imports
 * `m`, `mc`, `sig`, and `arg` from here.
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

// Re-export the enum values so category files don't need
// a second import from hookTypes.
export { MacroCategory, MacroKind };

// ─── m: primary macro builder ───────────────────────────────────

export function m(
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

// ─── mc: macro builder with MacroCategory.Custom ────────────────

/** Helper: build a macro definition with MacroCategory.Custom and a categoryDetail string */
export function mc(
  name: string,
  categoryDetail: string,
  kind: MacroKind,
  description: string,
  signatures: MacroSignatureDef[],
  opts?: {
    aliases?: string[];
    deprecated?: boolean;
    deprecationMessage?: string;
    children?: string[];
    parents?: string[];
    hasBody?: boolean;
    isNavigation?: boolean;
    isInclude?: boolean;
    isConditional?: boolean;
    isAssignment?: boolean;
    passageArgPosition?: number;
  },
): MacroDef {
  return m(name, MacroCategory.Custom, kind, description, signatures, { ...opts, categoryDetail });
}

// ─── sig: signature builder ─────────────────────────────────────

export function sig(args: MacroArgDef[], returnType?: string, description?: string): MacroSignatureDef {
  return { args, returnType, description };
}

// ─── arg: argument builder ──────────────────────────────────────

export function arg(name: string, type: string, required: boolean, opts?: Partial<Pick<MacroArgDef, 'variadic' | 'description'>>): MacroArgDef {
  return { name, type, required, variadic: opts?.variadic, description: opts?.description };
}
