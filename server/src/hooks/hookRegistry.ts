/**
 * Knot v2 — Hook Registry (DEPRECATED)
 *
 * This module has been replaced by FormatRegistry in formats/formatRegistry.ts.
 * The IFormatProvider/HookRegistry pattern has been superseded by the
 * FormatModule + capability bag architecture.
 *
 * All consumers should import FormatRegistry from '../formats/formatRegistry'.
 *
 * This file is kept only to avoid breaking any remaining import paths.
 * It will be removed in a future cleanup pass.
 */

export { FormatRegistry as HookRegistry } from '../formats/formatRegistry';
