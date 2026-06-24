//! Shared utility functions for the Knot VS Code extension.
//!
//! These helpers are used by both `extension.ts` and `navigation.ts`.
//! Previously duplicated across both files to avoid circular imports;
//! now centralized here as a single source of truth.

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';

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

// ---------------------------------------------------------------------------
// Knot-managed toolchain paths
// ---------------------------------------------------------------------------

//
// The extension manages the tweego binary and storyformats in VS Code's
// globalStorage directory. This gives us:
//
//   <globalStorage>/tweego/tweego.exe   — the managed tweego binary
//   <globalStorage>/storyformats/       — managed story formats (NOT next
//                                          to the binary, so tweego's
//                                          binary-sibling search finds
//                                          nothing and CWD overrides work)
//
// The globalStorage path is set once during activation via
// `setGlobalStoragePath()`, then all build requests automatically include
// the managed storyformats path so the server can set TWEEGO_PATH.

let _globalStoragePath: string | null = null;

/**
 * Set the extension's globalStorage path. Called once during activation.
 * This enables `getBuildRequestParams()` to include the managed
 * storyformats path in every build request.
 */
export function setGlobalStoragePath(globalStoragePath: string): void {
    _globalStoragePath = globalStoragePath;
}

/**
 * Get the path to the Knot-managed tweego binary, if it exists.
 * Returns `undefined` if tweego hasn't been downloaded yet.
 */
export function getManagedTweegoPath(): string | undefined {
    if (!_globalStoragePath) { return undefined; }
    const binaryName = process.platform === 'win32' ? 'tweego.exe' : 'tweego';
    const binPath = path.join(_globalStoragePath, 'tweego', binaryName);
    try {
        fs.accessSync(binPath, fs.constants.X_OK);
        return binPath;
    } catch {
        return undefined;
    }
}

/**
 * Get the path to the Knot-managed storyformats directory, if it exists.
 * Returns `undefined` if storyformats haven't been downloaded yet.
 *
 * The managed storyformats live in `<globalStorage>/storyformats/` —
 * deliberately NOT next to the tweego binary, so tweego's binary-sibling
 * search finds nothing and project-local `<workspace>/storyformats/`
 * overrides can take priority.
 */
export function getManagedStoryformatsPath(): string | undefined {
    if (!_globalStoragePath) { return undefined; }
    const sfPath = path.join(_globalStoragePath, 'storyformats');
    try {
        fs.accessSync(sfPath, fs.constants.R_OK);
        return sfPath;
    } catch {
        return undefined;
    }
}

// ---------------------------------------------------------------------------
// Build request params helper
// ---------------------------------------------------------------------------

/**
 * Build the request params object for `knot/build` and `knot/play` requests,
 * reading all build-related settings from the VS Code Settings UI.
 *
 * This is the SINGLE source of truth for how VS Code settings map to LSP
 * build params. All call sites (the Build command, the Build Task, the
 * auto-rebuild watcher, and Play Mode) MUST use this helper so that
 * settings like `knot.build.sourceDir` are consistently respected.
 *
 * Settings are only included in the params when they have a non-empty
 * value — the server treats absent fields as "use config fallback" and
 * empty strings as "use the default".
 *
 * Settings read:
 * - `knot.tweegoPath`        → `compiler_path`
 * - `knot.build.sourceDir`   → `source_dir`
 * - `knot.build.outputDir`   → `output_dir`
 * - `knot.storyformats.path` → `storyformats_path`
 *
 * @param workspaceUri The workspace root URI as a string.
 * @param startPassage Optional start passage name (for Play From Passage).
 */
export function getBuildRequestParams(
    workspaceUri: string,
    startPassage?: string,
): Record<string, string> {
    const config = vscode.workspace.getConfiguration('knot');

    const params: Record<string, string> = {
        workspace_uri: workspaceUri,
    };

    // Compiler path: prefer the user's setting, then the managed binary.
    const tweegoPathSetting = config.get<string>('tweegoPath') || '';
    if (tweegoPathSetting.trim()) {
        params.compiler_path = tweegoPathSetting;
    } else {
        const managedTweego = getManagedTweegoPath();
        if (managedTweego) {
            params.compiler_path = managedTweego;
        }
    }

    const sourceDir = config.get<string>('build.sourceDir') || '';
    if (sourceDir.trim()) {
        params.source_dir = sourceDir;
    }

    const outputDir = config.get<string>('build.outputDir') || '';
    if (outputDir.trim()) {
        params.output_dir = outputDir;
    }

    const storyformatsPath = config.get<string>('storyformats.path') || '';
    if (storyformatsPath.trim()) {
        params.storyformats_path = storyformatsPath;
    }

    // Always include the managed storyformats path so the server can set
    // TWEEGO_PATH. This is the fallback — project-local storyformats/ and
    // the user's setting both take priority (tweego searches CWD before
    // TWEEGO_PATH, and we put the user's setting first in TWEEGO_PATH).
    const managedSf = getManagedStoryformatsPath();
    if (managedSf) {
        params.managed_storyformats_path = managedSf;
    }

    if (startPassage) {
        params.start_passage = startPassage;
    }

    return params;
}

/**
 * Build the request params object for `knot/formats/refresh` requests,
 * reading the `knot.storyformats.path` setting from the VS Code Settings UI.
 *
 * This ensures the server's formats catalog respects the VS Code setting,
 * not just `.vscode/knot.json`. Used by the `Knot: Configure Story Formats`
 * command after path changes and after the "Browse for folder" flow.
 *
 * @param workspaceUri The workspace root URI as a string.
 */
export function getFormatsRefreshParams(
    workspaceUri: string,
): Record<string, string> {
    const config = vscode.workspace.getConfiguration('knot');

    const params: Record<string, string> = {
        workspace_uri: workspaceUri,
    };

    const storyformatsPath = config.get<string>('storyformats.path') || '';
    if (storyformatsPath.trim()) {
        params.storyformats_path = storyformatsPath;
    }

    return params;
}
