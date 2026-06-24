//! Custom LSP request handlers for story format management
//! (`knot/formats/list`, `knot/formats/refresh`).
//!
//! These handlers let the VS Code extension query and refresh the server's
//! catalog of installed story formats — discovered by scanning a storyformats
//! directory and parsing each `format.js` file inside it.
//!
//! ## Architecture
//!
//! The server does NOT maintain a list of download URLs for story formats.
//! The user's local copy is authoritative — they install formats by
//! downloading them into a directory, then point Knot at that directory
//! via the `knot.storyformats.path` VS Code setting (visible in the
//! Settings UI as a folder picker).
//!
//! Resolution order (first hit wins):
//! 1. `knot.storyformats.path` setting (highest priority)
//! 2. Project-local `.storyformats/` directory
//! 3. `<tweego_dir>/storyformats/` (where official Tweego releases ship)
//! 4. None — tweego's own search will run, and the build will likely fail
//!    with a clear "story format not found" message that we surface as a
//!    diagnostic.

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_formats::format_meta::{InstalledFormat, scan_storyformats_dir};

/// Map a format name string (as it appears in StoryData, e.g. "SugarCube")
/// to the tweego format ID (directory name, e.g. "sugarcube-2").
///
/// Returns `Err` with a JSON-RPC error if the format is unknown — used by
/// `knot_formats_list` to bail out cleanly when StoryData references an
/// unsupported format.
fn format_name_to_id(format_name: &str) -> Result<&'static str, tower_lsp::jsonrpc::Error> {
    match format_name {
        "SugarCube" => Ok("sugarcube-2"),
        "Harlowe" => Ok("harlowe-3"),
        "Chapbook" => Ok("chapbook-1"),
        "Snowman" => Ok("snowman-2"),
        _ => Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
            "Unknown story format '{}': only SugarCube, Harlowe, Chapbook, Snowman are supported",
            format_name
        ))),
    }
}

impl ServerState {
    /// `knot/formats/list` — return the catalog of installed story formats.
    ///
    /// If `path_override` is set in the params, the server scans that
    /// directory instead of the configured/resolved one. This is used by
    /// the "Browse for folder..." UI to preview what's in a directory
    /// before saving it as the configured path.
    pub async fn knot_formats_list(
        &self,
        params: KnotFormatsListParams,
    ) -> Result<KnotFormatsListResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Read project format info from StoryData (already parsed during indexing).
        // This lets the extension offer one-click download of the exact format
        // version the project needs, without asking the user to type a version.
        let project_format = inner
            .workspace
            .metadata
            .as_ref()
            .map(|m| format!("{:?}", m.format));
        let project_format_version = inner
            .workspace
            .metadata
            .as_ref()
            .and_then(|m| m.format_version.clone());
        let global_storage = inner.global_storage_path.clone();

        // Check if the project's needed format is already in the managed cache.
        let project_format_cached = match (&global_storage, &project_format, &project_format_version) {
            (Some(gs), Some(fmt), Some(ver)) => {
                let format_id = format_name_to_id(fmt)?;
                let format_js = gs
                    .join("storyformats")
                    .join(format!("{}@{}", format_id, ver))
                    .join(format_id)
                    .join("format.js");
                Some(format_js.exists())
            }
            _ => None,
        };

        let configured_path = inner
            .workspace
            .config
            .storyformats_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        // Path override: scan the requested directory without persisting.
        if let Some(ref override_path) = params.path_override {
            let path = std::path::PathBuf::from(override_path);
            if !path.is_dir() {
                return Ok(KnotFormatsListResponse {
                    resolved_dir: Some(override_path.clone()),
                    formats: Vec::new(),
                    configured_path,
                    project_format,
                    project_format_version,
                    project_format_cached,
                });
            }
            let formats = scan_storyformats_dir(&path);
            let entries: Vec<KnotFormatEntry> =
                formats.iter().map(format_to_entry).collect();
            return Ok(KnotFormatsListResponse {
                resolved_dir: Some(path.to_string_lossy().to_string()),
                formats: entries,
                configured_path,
                project_format,
                project_format_version,
                project_format_cached,
            });
        }

        // Default: return the cached catalog (or rebuild if empty).
        let formats = if inner.installed_formats.is_empty() {
            Vec::new()
        } else {
            inner
                .installed_formats
                .iter()
                .map(format_to_entry)
                .collect()
        };

        // The resolved_dir field is reconstructed from the cached catalog's
        // first entry's parent directory, or None if empty.
        let resolved_dir = inner
            .installed_formats
            .first()
            .and_then(|f| std::path::Path::new(&f.dir).parent())
            .map(|p| p.to_string_lossy().to_string());

        Ok(KnotFormatsListResponse {
            resolved_dir,
            formats,
            configured_path,
            project_format,
            project_format_version,
            project_format_cached,
        })
    }

    /// `knot/formats/refresh` — force re-scan of the storyformats directory.
    ///
    /// Re-resolves the storyformats directory using the current config and
    /// workspace root, re-scans it, and updates the cached catalog on
    /// `ServerStateInner`.
    ///
    /// The `storyformats_path` param (from the VS Code `knot.storyformats.path`
    /// setting) takes priority over the server's `.vscode/knot.json` config.
    pub async fn knot_formats_refresh(
        &self,
        params: KnotFormatsRefreshParams,
    ) -> Result<KnotFormatsRefreshResponse, tower_lsp::jsonrpc::Error> {
        // Take a read lock to copy what we need, then drop before scanning.
        let (config_storyformats_path, workspace_root_path, tweego_path) = {
            let inner = self.inner.read().await;
            let workspace_root = inner
                .workspace
                .root_uri
                .to_file_path()
                .ok();
            let tweego_path = inner.workspace.config.compiler_path.clone();
            (
                inner.workspace.config.storyformats_path.clone(),
                workspace_root,
                tweego_path,
            )
        };

        // Priority: VS Code setting (params.storyformats_path) > .vscode/knot.json
        // (config.storyformats_path) > auto-discovery.
        let configured_path = params
            .storyformats_path
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or(config_storyformats_path);

        // Resolve the storyformats directory using the layered discovery.
        let resolved = helpers::resolve_storyformats_dir(
            configured_path.as_deref(),
            workspace_root_path.as_deref(),
            tweego_path.as_deref(),
        );

        // Scan the resolved directory (or empty list if None).
        let formats: Vec<InstalledFormat> = match &resolved {
            Some(dir) => scan_storyformats_dir(dir),
            None => Vec::new(),
        };

        let format_count = formats.len();
        let resolved_dir_str = resolved
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        // Acquire the write lock to update the cached catalog.
        {
            let mut inner = self.inner.write().await;
            inner.installed_formats = formats;
        }

        tracing::info!(
            "Refreshed storyformats catalog: {} format(s) from {}",
            format_count,
            resolved_dir_str.as_deref().unwrap_or("(none resolved)")
        );

        Ok(KnotFormatsRefreshResponse {
            success: true,
            resolved_dir: resolved_dir_str,
            format_count,
            error: None,
        })
    }
}

/// Convert a server-side `InstalledFormat` into the LSP-serializable form.
fn format_to_entry(f: &InstalledFormat) -> KnotFormatEntry {
    KnotFormatEntry {
        name: f.meta.name.clone(),
        version: f.meta.version.clone(),
        description: f.meta.description.clone(),
        author: f.meta.author.clone(),
        license: f.meta.license.clone(),
        source: f.meta.source.clone(),
        url: f.meta.url.clone(),
        dir: f.dir.clone(),
        dir_name: f.dir_name.clone(),
    }
}
