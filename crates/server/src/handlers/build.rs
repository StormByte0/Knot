//! Custom LSP request handlers for the build pipeline (knot/build, knot/play, knot/compilerDetect).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use std::path::Path;

/// Force a path string to be relative by stripping leading separators and
/// Windows drive prefixes.
///
/// Rust's `PathBuf::join` has a footgun: joining an absolute path REPLACES
/// the base entirely (`PathBuf::from("/home/proj").join("/src")` == `/src`),
/// which would point tweego at the disk root instead of `<workspace>/src`.
/// This function strips any leading `/`, `\`, or `<drive>:` prefix so the
/// value is always treated as relative to the workspace root.
///
/// Examples:
///   `/src`        → `src`
///   `\src`        → `src`
///   `C:\src`      → `src`
///   `src`         → `src`  (unchanged)
///   `./src`       → `src`  (strips leading `./`)
fn force_relative(s: &str) -> String {
    let trimmed = s.trim();

    // Handle Windows drive prefix: `C:\src` or `C:/src`
    let after_drive = if trimmed.len() >= 2
        && trimmed.as_bytes()[1] == b':'
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
    {
        &trimmed[2..]
    } else {
        trimmed
    };

    // Strip leading path separators and `./` prefixes
    let mut start = 0;
    let bytes = after_drive.as_bytes();
    while start < bytes.len() {
        if bytes[start] == b'/' || bytes[start] == b'\\' {
            start += 1;
        } else if start + 1 < bytes.len()
            && bytes[start] == b'.'
            && (bytes[start + 1] == b'/' || bytes[start + 1] == b'\\')
        {
            start += 2;
        } else {
            break;
        }
    }

    after_drive[start..].to_string()
}

/// Map a `StoryFormat` enum to the directory ID that tweego expects in
/// storyformats folders.
fn format_to_id(format: &knot_core::passage::StoryFormat) -> &'static str {
    use knot_core::passage::StoryFormat;
    match format {
        StoryFormat::SugarCube => "sugarcube-2",
        StoryFormat::Harlowe => "harlowe-3",
        StoryFormat::Chapbook => "chapbook-1",
        StoryFormat::Snowman => "snowman-2",
        StoryFormat::Core => "sugarcube-2", // fallback
    }
}

/// Check if a directory looks like a toolchain directory rather than a
/// source directory.
///
/// Returns true if the directory contains:
/// - A `tweego` or `tweego.exe` binary (the tweego toolchain)
/// - A `storyformats/` subdirectory (tweego's bundled formats)
///
/// This is used to reject the common mistake of setting `knot.build.sourceDir`
/// to `tweego` when the user meant `src`. Without this check, tweego would
/// recursively scan the toolchain directory and pick up `format.js` files
/// as script passages, causing "Replacing existing passage" warnings and
/// SugarCube runtime errors.
fn is_toolchain_dir(path: &Path) -> bool {
    // Check for tweego binary
    let tweego_bin = path.join(if cfg!(windows) { "tweego.exe" } else { "tweego" });
    if tweego_bin.exists() {
        return true;
    }

    // Check for storyformats/ subdirectory
    let storyformats = path.join("storyformats");
    if storyformats.is_dir() {
        return true;
    }

    false
}

impl ServerState {
    /// `knot/build` — trigger project compilation.
    pub async fn knot_build(
        &self,
        params: KnotBuildParams,
    ) -> Result<KnotBuildResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/build: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let root_uri = inner.workspace.root_uri.clone();
        let config = inner.workspace.config.clone();
        // Read format + version from StoryData for versioned format cache lookup.
        // This MUST happen before drop(inner) since workspace.metadata is only
        // accessible while holding the read lock.
        let story_format = inner.workspace.metadata.as_ref().map(|m| m.format.clone());
        let format_version = inner.workspace.metadata.as_ref()
            .and_then(|m| m.format_version.clone());
        let global_storage_path = inner.global_storage_path.clone();
        drop(inner);

        let root_path = match root_uri.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                return Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec!["Workspace root is not a valid file path".to_string()],
                });
            }
        };

        // ── Resolve tweego binary ─────────────────────────────────────────
        //
        // Priority:
        //   1. VS Code setting `knot.tweegoPath` (params.compiler_path)
        //   2. `.vscode/knot.json` compiler_path
        //   3. PATH lookup (which_compiler)
        //   4. Managed binary: <globalStorage>/tweego/tweego[.exe]
        //
        // The managed binary has NO storyformats next to it (the download
        // relocates them to <globalStorage>/storyformats/), so tweego's
        // binary-sibling search finds nothing. This ensures CWD overrides
        // and the managed cache are the only sources for storyformats.
        let compiler_path = if let Some(ref ext_path) = params.compiler_path {
            Some(std::path::PathBuf::from(ext_path))
        } else if let Some(ref path) = config.compiler_path {
            Some(path.clone())
        } else if let Some(ref p) = helpers::which_compiler() {
            Some(p.clone())
        } else {
            // Check managed binary
            if let Some(ref gs) = global_storage_path {
                let managed_bin = gs.join("tweego").join(if cfg!(windows) { "tweego.exe" } else { "tweego" });
                if managed_bin.exists() {
                    Some(managed_bin)
                } else {
                    None
                }
            } else {
                None
            }
        };

        let Some(compiler_path) = compiler_path else {
            return Ok(KnotBuildResponse {
                success: false,
                output_path: None,
                errors: vec![
                    "No Tweego compiler found. Options:\n\
                     1. Install Tweego and add it to PATH\n\
                     2. Set 'knot.tweegoPath' in Settings to point at your tweego binary\n\
                     3. Use 'Knot: Configure Build Toolchain' to download Tweego automatically"
                        .to_string(),
                ],
            });
        };

        // Log the actual tweego binary path so users can verify which binary
        // will be invoked. This is separate from TWEEGO_PATH (which is an env
        // var telling tweego where to find story formats, not a binary path).
        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: format!("Knot: Tweego binary: {}", compiler_path.display()),
                is_error: false,
            })
            .await;

        // ── Resolve source directory ─────────────────────────────────────
        //
        // Priority:
        //   1. VS Code setting `knot.build.sourceDir` (or .vscode/knot.json)
        //   2. Auto-detect: <workspace>/src/ if it exists
        //   3. Fallback: workspace root
        //
        // VALIDATION: If the resolved source dir looks like a toolchain dir
        // (contains tweego.exe/tweego binary or a storyformats/ subdirectory),
        // reject it and fall back to auto-detect. This catches the common
        // mistake of setting sourceDir to "tweego" when the user meant "src".
        let source_dir_setting = params
            .source_dir
            .as_ref()
            .filter(|s| !s.is_empty())
            .or_else(|| config.build.source_dir.as_ref().filter(|s| !s.is_empty()));

        let mut source_path = match source_dir_setting {
            Some(sd) => {
                let relative = force_relative(sd);
                root_path.join(&relative)
            }
            None => {
                let auto_src = root_path.join("src");
                if auto_src.is_dir() {
                    auto_src
                } else {
                    root_path.clone()
                }
            }
        };

        // Validate: reject toolchain directories.
        // If the source path contains a tweego binary or a storyformats/
        // subdirectory, it's a toolchain dir, not a source dir.
        if is_toolchain_dir(&source_path) {
            let warning_msg = format!(
                "Knot: WARNING: Source directory '{}' appears to be a toolchain directory \
                 (contains tweego binary or storyformats/ folder), not a source directory. \
                 Falling back to auto-detect.",
                source_path.display()
            );
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: warning_msg,
                    is_error: true,
                })
                .await;

            // Fall back to auto-detect
            let auto_src = root_path.join("src");
            if auto_src.is_dir() {
                source_path = auto_src;
            } else {
                source_path = root_path.clone();
            }
        }

        // Emit the source path to the build output stream.
        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: format!("Knot: Compiling source from: {}", source_path.display()),
                is_error: false,
            })
            .await;

        // ── Resolve storyformats ─────────────────────────────────────────
        //
        // Architecture: settings → CWD → managed cache → error
        //
        // Tweego's internal search order:
        //   1. <tweego_binary_dir>/storyformats/  (EMPTY for managed binary)
        //   2. <home>/storyformats/
        //   3. <cwd>/storyformats/                (PROJECT-LOCAL OVERRIDE)
        //   4. TWEEGO_PATH env var                (managed fallback)
        //
        // Our resolution:
        //   a. If <workspace>/storyformats/ exists → CWD override (tweego finds
        //      it via #3). Don't set TWEEGO_PATH.
        //   b. Else if knot.storyformats.path setting is set → set TWEEGO_PATH
        //   c. Else if <globalStorage>/storyformats/<id>@<ver>/ exists → set
        //      TWEEGO_PATH to the versioned cache dir
        //   d. Else → error with download hint

        let cwd_storyformats = root_path.join("storyformats");
        let cwd_has_override = cwd_storyformats.is_dir();

        let user_storyformats = params
            .storyformats_path
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| config.storyformats_path.clone());

        // Versioned managed cache: <globalStorage>/storyformats/sugarcube-2@2.37.0/
        // Validate that format.js actually exists inside, not just that the
        // directory exists — a failed download can leave an empty directory.
        let versioned_managed = match (&global_storage_path, &story_format, &format_version) {
            (Some(gs), Some(fmt), Some(ver)) => {
                let format_id = format_to_id(fmt);
                let versioned_dir = gs
                    .join("storyformats")
                    .join(format!("{}@{}", format_id, ver));
                // Check for the actual format.js file inside the format-id subdir
                let format_js = versioned_dir.join(format_id).join("format.js");
                if format_js.exists() {
                    Some(versioned_dir)
                } else {
                    None
                }
            }
            _ => None,
        };

        // Build the diagnostic message and determine TWEEGO_PATH
        let (resolution_msg, tweego_path_value) = if cwd_has_override {
            (
                format!(
                    "Knot: Story formats: using project-local override at {} (CWD storyformats/ takes priority)",
                    cwd_storyformats.display()
                ),
                None, // Don't set TWEEGO_PATH — tweego finds CWD automatically
            )
        } else if let Some(ref vm) = versioned_managed {
            (
                format!(
                    "Knot: Story formats: using managed cache at {} (format={} version={})",
                    vm.display(),
                    story_format.map(|f| format!("{:?}", f)).unwrap_or_default(),
                    format_version.as_deref().unwrap_or("?")
                ),
                Some(vm.to_string_lossy().to_string()),
            )
        } else if let Some(ref us) = user_storyformats {
            if us.is_dir() {
                (
                    format!(
                        "Knot: Story formats: using configured path {} (knot.storyformats.path setting)",
                        us.display()
                    ),
                    Some(us.to_string_lossy().to_string()),
                )
            } else {
                (
                    format!(
                        "Knot: WARNING: Configured storyformats path '{}' does not exist",
                        us.display()
                    ),
                    None,
                )
            }
        } else {
            // Nothing resolved — build will likely fail.
            // Include the managed cache path in the error so the user knows
            // where formats should go.
            let hint = match (&story_format, &format_version, &global_storage_path) {
                (Some(fmt), Some(ver), Some(gs)) => {
                    let format_id = format_to_id(fmt);
                    format!(
                        " — project needs {} v{} but it's not in the managed cache.\n\
                         Expected at: {}\\storyformats\\{}@{}\\{}\\format.js\n\
                         Use 'Knot: Configure Story Formats' to download it.",
                        format_id, ver,
                        gs.display(),
                        format_id, ver, format_id
                    )
                }
                (Some(fmt), Some(ver), None) => {
                    let format_id = format_to_id(fmt);
                    format!(
                        " — project needs {} v{} but extension global storage is not available.\n\
                         Use 'Knot: Configure Story Formats' to download it.",
                        format_id, ver
                    )
                }
                _ => " — no StoryData format/version detected. Is StoryData passage present?".to_string(),
            };
            (
                format!("Knot: No story formats directory resolved{}", hint),
                None,
            )
        };

        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: resolution_msg,
                is_error: false,
            })
            .await;

        // ── Determine output directory ───────────────────────────────────
        let output_dir_name = params
            .output_dir
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.as_str())
            .unwrap_or(&config.build.output_dir);
        let output_dir = root_path.join(output_dir_name);
        std::fs::create_dir_all(&output_dir).ok();

        let output_file = output_dir.join("index.html");

        // Build the command arguments
        let mut args: Vec<String> = Vec::new();

        // If a start passage is specified, add --start flag
        if let Some(ref start_passage) = params.start_passage {
            args.push("--start".to_string());
            args.push(start_passage.clone());
        }

        args.push("-o".to_string());
        args.push(output_file.to_string_lossy().to_string());
        args.extend(config.build.flags.iter().cloned());
        // Source directory must be the LAST argument
        args.push(source_path.to_string_lossy().to_string());

        tracing::info!("Build command: {} {}", compiler_path.display(), args.join(" "));

        // Run the compiler with cwd set to the workspace root.
        //
        // TWEEGO_PATH is set ONLY when we resolved a storyformats directory
        // that tweego can't find via its own CWD search (i.e. the managed
        // cache or user-configured path). When CWD override is active,
        // tweego finds <workspace>/storyformats/ automatically — no
        // TWEEGO_PATH needed.
        let mut command = tokio::process::Command::new(&compiler_path);
        command.args(&args).current_dir(&root_path);

        if let Some(ref tp) = tweego_path_value {
            command.env("TWEEGO_PATH", tp);
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: format!("Knot: Story formats search path (TWEEGO_PATH) = {}", tp),
                    is_error: false,
                })
                .await;
        }

        let output = command.output().await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Stream build output to the client
                for line in stdout.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: false,
                        })
                        .await;
                }
                for line in stderr.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: true,
                        })
                        .await;
                }

                if output.status.success() {
                    tracing::info!("Build succeeded: {}", output_file.display());
                    Ok(KnotBuildResponse {
                        success: true,
                        output_path: Some(output_file.to_string_lossy().to_string()),
                        errors: Vec::new(),
                    })
                } else {
                    let error_lines: Vec<String> = stderr.lines().map(|l| l.to_string()).collect();
                    tracing::warn!("Build failed: {}", error_lines.join("; "));
                    Ok(KnotBuildResponse {
                        success: false,
                        output_path: None,
                        errors: if error_lines.is_empty() {
                            vec!["Build failed with no error output".to_string()]
                        } else {
                            error_lines
                        },
                    })
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute compiler: {}", e);
                Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec![format!("Failed to execute compiler: {}", e)],
                })
            }
        }
    }

    /// `knot/play` — compile the project and return the HTML path for preview.
    pub async fn knot_play(
        &self,
        params: KnotPlayParams,
    ) -> Result<KnotPlayResponse, tower_lsp::jsonrpc::Error> {
        // Build first
        let build_result = self.knot_build(KnotBuildParams {
            workspace_uri: params.workspace_uri.clone(),
            start_passage: params.start_passage.clone(),
            compiler_path: params.compiler_path.clone(),
            source_dir: params.source_dir.clone(),
            output_dir: params.output_dir.clone(),
            storyformats_path: params.storyformats_path.clone(),
            managed_storyformats_path: params.managed_storyformats_path.clone(),
        }).await?;

        if build_result.success {
            Ok(KnotPlayResponse {
                html_path: build_result.output_path,
                error: None,
            })
        } else {
            Ok(KnotPlayResponse {
                html_path: None,
                error: Some(build_result.errors.join("\n")),
            })
        }
    }

    /// `knot/compilerDetect` — detect whether a Twine compiler is available.
    pub async fn knot_compiler_detect(
        &self,
        params: KnotCompilerDetectParams,
    ) -> Result<KnotCompilerDetectResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/compilerDetect: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let config = inner.workspace.config.clone();
        drop(inner);

        // Check configured path first
        if let Some(ref path) = config.compiler_path
            && path.exists() {
                return Ok(KnotCompilerDetectResponse {
                    compiler_found: true,
                    compiler_name: Some("tweego".to_string()),
                    compiler_version: helpers::detect_compiler_version(path).await,
                    compiler_path: Some(path.to_string_lossy().to_string()),
                });
            }

        // Check PATH
        if let Some(path) = helpers::which_compiler() {
            return Ok(KnotCompilerDetectResponse {
                compiler_found: true,
                compiler_name: Some("tweego".to_string()),
                compiler_version: helpers::detect_compiler_version(&path).await,
                compiler_path: Some(path.to_string_lossy().to_string()),
            });
        }

        Ok(KnotCompilerDetectResponse {
            compiler_found: false,
            compiler_name: None,
            compiler_version: None,
            compiler_path: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_force_relative_plain_relative() {
        assert_eq!(force_relative("src"), "src");
    }

    #[test]
    fn test_force_relative_leading_slash() {
        assert_eq!(force_relative("/src"), "src");
    }

    #[test]
    fn test_force_relative_leading_backslash() {
        assert_eq!(force_relative("\\src"), "src");
    }

    #[test]
    fn test_force_relative_windows_drive() {
        assert_eq!(force_relative("C:\\src"), "src");
        assert_eq!(force_relative("D:/src"), "src");
    }

    #[test]
    fn test_force_relative_dot_slash() {
        assert_eq!(force_relative("./src"), "src");
        assert_eq!(force_relative(".\\src"), "src");
    }

    #[test]
    fn test_force_relative_nested_path() {
        assert_eq!(force_relative("/a/b/c"), "a/b/c");
        assert_eq!(force_relative("a/b/c"), "a/b/c");
        assert_eq!(force_relative("C:\\a\\b"), "a\\b");
    }

    #[test]
    fn test_force_relative_whitespace() {
        assert_eq!(force_relative("  /src  "), "src");
    }

    #[test]
    fn test_force_relative_empty() {
        assert_eq!(force_relative(""), "");
    }
}


