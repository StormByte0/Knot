//! Compiler detection, version detection, and storyformats directory resolution.
//!
//! ## Storyformats resolution
//!
//! The storyformats directory contains subdirectories like `sugarcube-2/`,
//! `harlowe-3/`, etc. — each with a `format.js` file. Tweego finds this
//! directory via its own search logic:
//!
//! 1. `<cwd>/storyformats/`
//! 2. `<tweego_binary_dir>/storyformats/`
//! 3. System paths
//!
//! We do NOT pass any flag to tweego for storyformats — tweego's `--head`
//! flag is for HTML head content, not storyformats. Instead, we resolve
//! the storyformats directory ourselves (for diagnostic logging and for
//! the `knot/formats/*` UI handlers) and rely on tweego's own search at
//! build time.
//!
//! Resolution order (first hit wins):
//!
//! 1. **Configured path** — `knot.build.storyformatsPath` VS Code setting or
//!    `storyformats_path` field in `.vscode/knot.json`. Highest priority.
//! 2. **Project-local** — `<workspace_root>/storyformats/`. Matches
//!    tweego's `<cwd>/storyformats/` search when cwd is the workspace root.
//! 3. **Tweego binary sibling** — `<tweego_dir>/storyformats/`. Where
//!    storyformats live when the user installed tweego from a release zip.
//! 4. **None** — return `None`. The build will rely on tweego's own search.
//!
//! Knot does NOT maintain a list of download URLs for storyformats. The
//! user's local copy is authoritative. If no storyformats are found, the
//! `Knot: Configure Story Formats` command lets the user point at a
//! directory or download one official format.

use std::path::{Path, PathBuf};

/// Search for the Tweego compiler on the system PATH.
///
/// On Unix systems, uses `which` to locate the binary.
/// On Windows, uses `where` instead (the `which` command does not exist).
/// Falls back to trying direct execution with `--version` if the
/// system locator is unavailable.
pub(crate) fn which_compiler() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["tweego.exe"]
    } else {
        &["tweego"]
    };

    // Use the platform-appropriate locator command
    let locator = if cfg!(windows) { "where" } else { "which" };

    for name in candidates {
        if let Ok(output) = std::process::Command::new(locator).arg(name).output()
            && output.status.success()
        {
            // `where` on Windows may return multiple lines; take the first.
            let path_str = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let path = PathBuf::from(&path_str);
            if path.exists() {
                return Some(path);
            }
        }
    }

    // Fallback: try direct execution — if the binary is on PATH,
    // running it with --version will succeed.
    for name in candidates {
        if std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(PathBuf::from(name));
        }
    }

    None
}

/// Detect the version string of a compiler by running `--version`.
///
/// Returns `None` if the compiler cannot be executed or exits non-zero.
/// This is best-effort — some tweego builds exit non-zero on `--version`
/// when storyformats cannot be found, in which case the caller should
/// still report `compiler_found: true` with `compiler_version: None`.
pub(crate) async fn detect_compiler_version(path: &Path) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout);
        // Take the first line of output as the version string
        Some(version.lines().next().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Resolve the storyformats directory using the discovery chain.
///
/// Returns the absolute path to the directory containing installed story
/// format subdirectories (each with its own `format.js` file). Returns
/// `None` if no candidate directory exists.
///
/// Resolution order:
///   1. Configured path — `knot.build.storyformatsPath` setting or `.vscode/knot.json`
///   2. None — caller falls back to managed cache or error
///
/// Note: We do NOT check the tweego binary's sibling directory or a
/// project-local `<workspace>/storyformats/` folder. Story formats live
/// exclusively in the extension-managed folder (`<globalStorage>/storyformats/`)
/// or a user-configured path. This keeps the workspace purely game files.
pub(crate) fn resolve_storyformats_dir(
    configured_path: Option<&Path>,
    _workspace_root: Option<&Path>,
    _tweego_path: Option<&Path>,
) -> Option<PathBuf> {
    // 1. Configured path (highest priority)
    if let Some(p) = configured_path {
        if p.is_dir() {
            tracing::debug!("Resolved storyformats dir from config: {}", p.display());
            return Some(p.to_path_buf());
        }
        tracing::debug!(
            "Configured storyformats path does not exist or is not a directory: {}",
            p.display()
        );
    }

    // 2. No resolution — caller falls back to managed cache or error.
    tracing::debug!("No storyformats directory resolved from config");
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_with_configured_path_existing() {
        let temp = tempfile::tempdir().unwrap();
        let result = resolve_storyformats_dir(Some(temp.path()), None, None);
        assert_eq!(result, Some(temp.path().to_path_buf()));
    }

    #[test]
    fn test_resolve_with_configured_path_nonexistent_falls_through() {
        let result =
            resolve_storyformats_dir(Some(Path::new("/nonexistent/path/abc123")), None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_does_not_use_project_local() {
        // Project-local <workspace>/storyformats/ is intentionally NOT
        // checked — the workspace is purely game files, formats live in
        // the managed folder.
        let workspace = tempfile::tempdir().unwrap();
        let local = workspace.path().join("storyformats");
        std::fs::create_dir_all(&local).unwrap();

        let result = resolve_storyformats_dir(None, Some(workspace.path()), None);
        assert_eq!(
            result, None,
            "Should NOT resolve from project-local storyformats/"
        );
    }

    #[test]
    fn test_resolve_does_not_use_tweego_sibling() {
        // We intentionally do NOT check the tweego binary's sibling directory.
        let tweego_dir = tempfile::tempdir().unwrap();
        let tweego_bin = tweego_dir.path().join("tweego");
        let sibling = tweego_dir.path().join("storyformats");
        std::fs::create_dir_all(&sibling).unwrap();

        let result = resolve_storyformats_dir(None, None, Some(&tweego_bin));
        assert_eq!(
            result, None,
            "Should NOT resolve from tweego binary sibling"
        );
    }

    #[test]
    fn test_resolve_returns_none_when_nothing_matches() {
        let result = resolve_storyformats_dir(None, None, None);
        assert_eq!(result, None);
    }
}
