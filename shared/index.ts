/**
 * Knot v2 — Shared Module Re-exports
 *
 * Types and enums shared between client and server.
 * Neither client nor server may import from each other —
 * they communicate through the LSP protocol.
 * Shared types are the common vocabulary.
 */

export { KnotStatus, KnotErrorCode } from './enums';
// TODO: export protocol types from ./protocol
