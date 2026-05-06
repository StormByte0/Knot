/**
 * Knot v2 — Hook Type Definitions (Enums)
 *
 * This file defines ONLY the enums that represent the TWINE ENGINE
 * base layer — universal concepts shared by ALL story formats.
 *
 * Format-specific categories are provided by the active adapter
 * through IFormatProvider sub-interfaces. Core NEVER hardcodes
 * format-specific enum values.
 *
 * CRITICAL RULES:
 *   - No format-specific values in these enums
 *   - Every enum MUST have a `Custom` catch-all for format extensions
 *   - Core files use only these enums + adapter-provided data
 *   - Formats may define their own sub-categories, reported via
 *     the adapter's getCustomTypes() or equivalent
 */

// ─── Macro Classification ───────────────────────────────────────

/**
 * Universal macro categories shared across formats.
 * Format-specific categories are represented as Custom + string label
 * via the adapter's MacroDefinition.categoryDetail field.
 */
export enum MacroCategory {
  Navigation = 'navigation',
  Output = 'output',
  Control = 'control',
  Variable = 'variable',
  Styling = 'styling',
  System = 'system',
  Utility = 'utility',
  Custom = 'custom',
}

// ─── Macro Kind (Changer vs Command vs Instant) ─────────────────

/**
 * How a macro relates to its body content.
 * This is universal across formats:
 *
 * - Changers: attach to a hook/body  (SugarCube: <<if>>...<</if>>, Harlowe: (if:)[...])
 * - Commands: standalone action      (SugarCube: <<goto>>, Harlowe: (go-to:))
 * - Instants: silent side-effect     (SugarCube: <<set>>, Harlowe: (set:))
 */
export enum MacroKind {
  Changer = 'changer',
  Command = 'command',
  Instant = 'instant',
}

// ─── Macro Body Style ───────────────────────────────────────────

/**
 * How a format's macros delimit their body content.
 * Determines how the parser assembles macro body AST nodes.
 *
 * 'close-tag' — SugarCube: body ends at <</name>> close tag
 * 'hook'      — Harlowe: body is [...] immediately after the macro call
 * 'inline'    — Chapbook/Snowman/Fallback: no macro bodies at all
 */
export enum MacroBodyStyle {
  CloseTag = 'close-tag',
  Hook = 'hook',
  Inline = 'inline',
}

// ─── Passage Classification (Twine Engine Only) ─────────────────

/**
 * Passage types known at the TWINE ENGINE level.
 * These are universal — every Twine story has these.
 *
 * Format-specific passage types (Widget, Header, Footer, Init, etc.)
 * are provided by the adapter via IPassageProvider.getPassageTypes()
 * and classified via IPassageProvider.classifyPassage().
 *
 * Core checks [script] and [stylesheet] Twee 3 spec tags first,
 * then delegates to the adapter for format-specific classification.
 */
export enum PassageType {
  /** Normal story passage — the default */
  Story = 'story',
  /** Twee 3 spec: [stylesheet] tag — CSS passage */
  Stylesheet = 'stylesheet',
  /** Twee 3 spec: [script] tag — JavaScript passage */
  Script = 'script',
  /** The starting passage (e.g. "Start") */
  Start = 'start',
  /** StoryData metadata passage (JSON, format auto-detection) */
  StoryData = 'storydata',
  /** Format-specific passage type — adapter provides details */
  Custom = 'custom',
}

// ─── Passage Kind (for classifyPassage) ──────────────────────────

/**
 * The fundamental kind of passage, used by classifyPassage().
 * Core recognizes 'script' and 'stylesheet' from Twee 3 spec tags
 * before calling the adapter. The adapter handles format-specific kinds.
 */
export enum PassageKind {
  /** Normal story passage (default, safe fallback) */
  Markup = 'markup',
  /** JavaScript passage (Twee 3 spec: [script] tag) */
  Script = 'script',
  /** CSS passage (Twee 3 spec: [stylesheet] tag) */
  Stylesheet = 'stylesheet',
  /** Format-specific special passage (widget, header, etc.) */
  Special = 'special',
}

// ─── Link Classification ────────────────────────────────────────

/**
 * Link types known at the Twine Engine level.
 * Core detects [[...]] boundaries. The adapter interprets the content
 * and classifies the link via ILinkProvider.resolveLinkBody().
 *
 * Passage and External are universal. Format-specific link types
 * use Custom with details from the adapter.
 */
export enum LinkKind {
  /** Internal passage link */
  Passage = 'passage',
  /** External URL */
  External = 'external',
  /** Format-specific link type */
  Custom = 'custom',
}

// ─── Passage Reference Kind ─────────────────────────────────────

/**
 * How a passage is referenced from within a passage body.
 * Core NEVER detects these — the format's extractPassageRefs()
 * is the single source of truth for all passage references.
 *
 * 'link'     — [[ ]] syntax (every format has this)
 * 'macro'    — format macro (<<goto>>, (go-to:), etc.)
 * 'api'      — JavaScript API call (Engine.play(), story.show(), etc.)
 * 'implicit' — implicit reference (data-passage, {embed passage:}, etc.)
 */
export enum PassageRefKind {
  Link = 'link',
  Macro = 'macro',
  API = 'api',
  Implicit = 'implicit',
}
