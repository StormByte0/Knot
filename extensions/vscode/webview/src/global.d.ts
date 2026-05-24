//! Global type declarations for the Knot Story Map webview.
//!
//! These ambient declarations make types available throughout the webview
//! without explicit imports.

// ---------------------------------------------------------------------------
// VS Code Webview API — provided by the VS Code webview host
// ---------------------------------------------------------------------------

interface VsCodeApi {
  postMessage(msg: any): void;
  getState(): unknown;
  setState(state: unknown): void;
}

declare function acquireVsCodeApi(): VsCodeApi;
