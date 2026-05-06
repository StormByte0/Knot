/**
 * Knot v2 — Shared Enums
 *
 * Status codes, error codes, and other enumerations shared
 * between client and server via the LSP protocol.
 *
 * These are NOT the same as hook enums (hookTypes.ts).
 * Hook enums define the core↔format boundary vocabulary.
 * These enums define the client↔server boundary vocabulary.
 */

/**
 * Server status values reported to the client.
 */
export enum KnotStatus {
  Starting = 'starting',
  Running = 'running',
  Stopping = 'stopping',
  Error = 'error',
  Idle = 'idle',
}

/**
 * Error codes for custom Knot protocol messages.
 */
export enum KnotErrorCode {
  Unknown = -32000,
  FormatNotRegistered = -32001,
  NoActiveFormat = -32002,
  WorkspaceNotIndexed = -32003,
  TweegoNotFound = -32004,
  BuildFailed = -32005,
}
