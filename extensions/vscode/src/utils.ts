//! Shared utility functions for the Knot VS Code extension.
//!
//! These helpers are used by both `extension.ts` and `navigation.ts`.
//! Previously duplicated across both files to avoid circular imports;
//! now centralized here as a single source of truth.

/** All recognized Twee language IDs in the extension. */
export const TWEE_LANGUAGE_IDS = ['twee', 'twee-sugarcube', 'twee-harlowe', 'twee-chapbook', 'twee-snowman'];

/** Check whether a language ID is any Twee variant. */
export function isTweeLanguage(languageId: string): boolean {
    return TWEE_LANGUAGE_IDS.includes(languageId);
}

/**
 * Extract the passage name from a `::` header line.
 *
 * A Twee passage header has the form:
 *   `:: Name [tag1 tag2] {"position":"100,200","size":"200,150"}`
 *
 * This function strips the `::` prefix, removes any `[tag]` blocks,
 * removes any `{JSON}` metadata blocks, and trims whitespace — matching
 * the Rust-side `extract_passage_name()` in `knot_formats::header`.
 */
export function extractPassageName(headerLine: string): string {
    // Strip the `::` prefix
    let name = headerLine.replace(/^::\s*/, '');

    // Strip JSON metadata blocks `{...}` — handle nested braces
    name = stripJsonBlock(name);

    // Strip tag blocks `[...]`
    name = stripTagBlock(name);

    return name.trim();
}

/**
 * Remove the first `{...}` JSON metadata block from a string.
 * Uses brace counting to handle nested objects, and validates the
 * extracted JSON with a parse check before removing.
 */
export function stripJsonBlock(s: string): string {
    const start = s.indexOf('{');
    if (start < 0) { return s; }

    let depth = 0;
    for (let i = start; i < s.length; i++) {
        if (s[i] === '{') { depth++; }
        else if (s[i] === '}') {
            depth--;
            if (depth === 0) {
                // Validate that the extracted block is valid JSON
                const candidate = s.substring(start, i + 1);
                try {
                    JSON.parse(candidate);
                    // Valid JSON — remove it
                    return s.substring(0, start) + s.substring(i + 1);
                } catch {
                    // Not valid JSON — leave as-is
                    return s;
                }
            }
        }
    }
    return s;
}

/**
 * Remove the first `[...]` tag block from a string.
 * Only strips if the block contains no nested brackets (simple tags).
 */
export function stripTagBlock(s: string): string {
    const start = s.indexOf('[');
    if (start < 0) { return s; }
    const end = s.indexOf(']', start);
    if (end < 0) { return s; }
    return s.substring(0, start) + s.substring(end + 1);
}
