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
///
/// This is used to reject the common mistake of setting `knot.build.sourceDir`
/// to `tweego` when the user meant `src`. Without this check, tweego would
/// recursively scan the toolchain directory and pick up `format.js` files
/// as script passages, causing "Replacing existing passage" warnings and
/// SugarCube runtime errors.
fn is_toolchain_dir(path: &Path) -> bool {
    // Check for tweego binary
    let tweego_bin = path.join(if cfg!(windows) {
        "tweego.exe"
    } else {
        "tweego"
    });
    if tweego_bin.exists() {
        return true;
    }

    false
}

/// Extract the story title text from the workspace's StoryTitle passage.
///
/// Joins all text blocks in the first StoryTitle passage found across
/// all documents, trimmed. Returns `None` if no StoryTitle passage exists
/// or if its body is empty.
fn extract_story_title(workspace: &knot_core::workspace::Workspace) -> Option<String> {
    use knot_core::passage::Block;
    let (_doc, passage) = workspace.find_passage("StoryTitle")?;
    let title: String = passage
        .body
        .iter()
        .filter_map(|block| match block {
            Block::Text { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if title.is_empty() { None } else { Some(title) }
}

/// Sanitize a story title for use as a filename.
///
/// Replaces characters that are invalid in filenames on Windows, macOS,
/// and Linux with underscores. Truncates to 80 characters to avoid
/// filesystem path length issues.
fn sanitize_filename(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_control()
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let truncated = sanitized.chars().take(80).collect::<String>();
    if truncated.trim().is_empty() {
        "index".to_string()
    } else {
        truncated
    }
}

/// Extract a numeric stat value from a tweego `-l` output line.
///
/// Tweego emits lines like `Passages: 42 | Words: 12345`. This helper
/// finds the label (e.g. `"Passages:"`) and returns the number that
/// follows it. Returns `None` if the label isn't found or the value
/// isn't a valid integer.
fn extract_stat(line: &str, label: &str) -> Option<String> {
    let pos = line.find(label)?;
    let after_label = &line[pos + label.len()..];
    let token = after_label
        .split(|c: char| c == '|' || c == ',' || c.is_whitespace())
        .find(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()))?;
    Some(token.to_string())
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
                    params.workspace_uri,
                    root
                );
            }
        }

        let root_uri = inner.workspace.root_uri.clone();
        let config = inner.workspace.config.clone();
        // Read format + version from StoryData for versioned format cache lookup.
        // Also extract StoryTitle for output filename derivation.
        // All of this MUST happen before drop(inner) since workspace state is
        // only accessible while holding the read lock.
        let story_format = inner.workspace.metadata.as_ref().map(|m| m.format.clone());
        let format_version = inner
            .workspace
            .metadata
            .as_ref()
            .and_then(|m| m.format_version.clone());
        let story_title = extract_story_title(&inner.workspace);
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
        //   1. VS Code setting `knot.build.tweegoPath` (params.compiler_path)
        //   2. `.vscode/knot.json` compiler_path
        //   3. PATH lookup (which_compiler)
        //   4. Managed binary: <globalStorage>/tweego/tweego[.exe]
        let compiler_path = if let Some(ref ext_path) = params.compiler_path {
            Some(std::path::PathBuf::from(ext_path))
        } else if let Some(ref path) = config.compiler_path {
            Some(path.clone())
        } else if let Some(ref p) = helpers::which_compiler() {
            Some(p.clone())
        } else {
            // Check managed binary
            if let Some(ref gs) = global_storage_path {
                let managed_bin = gs.join("tweego").join(if cfg!(windows) {
                    "tweego.exe"
                } else {
                    "tweego"
                });
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
                     2. Set 'knot.build.tweegoPath' in Settings to point at your tweego binary\n\
                     3. Use 'Knot: Configure Build Toolchain' to download Tweego automatically"
                        .to_string(),
                ],
            });
        };

        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: format!("Knot: Tweego binary: {}", compiler_path.display()),
                is_error: false,
            })
            .await;

        // ── Resolve source directory ─────────────────────────────────────
        //
        // Architecture (simplified): the workspace IS the source directory.
        // Users put all their game files (.twee, .js, .css, assets) directly
        // in the workspace. Story formats live separately in the extension-
        // managed folder, so there's no risk of format.js getting bundled
        // as a passage.
        //
        // The `knot.build.sourceDir` setting still works as an explicit
        // override for users who want to use a subdirectory, but there's
        // no more `src/` auto-detection.
        let source_path = match params
            .source_dir
            .as_ref()
            .filter(|s| !s.is_empty())
            .or_else(|| config.build.source_dir.as_ref().filter(|s| !s.is_empty()))
        {
            Some(sd) => {
                let relative = force_relative(sd);
                root_path.join(&relative)
            }
            None => root_path.clone(),
        };

        // Validate: reject toolchain directories (contains a tweego binary).
        // This catches the mistake of pointing sourceDir at the tweego folder.
        if is_toolchain_dir(&source_path) {
            let warning_msg = format!(
                "Knot: WARNING: Source directory '{}' appears to be a toolchain directory \
                 (contains tweego binary), not a source directory. Using workspace root instead.",
                source_path.display()
            );
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: warning_msg,
                    is_error: true,
                })
                .await;
            // Override: use workspace root
            let fallback = root_path.clone();
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: format!("Knot: Compiling source from: {}", fallback.display()),
                    is_error: false,
                })
                .await;
            // Use fallback for the rest of the function
            // (We can't reassign source_path in this scope due to the match
            //  arms, so we'll use fallback directly below.)

            // ── Resolve story formats ────────────────────────────────────
            let (resolution_msg, tweego_path_value) = self
                .resolve_story_formats(
                    &params,
                    &config,
                    &story_format,
                    &format_version,
                    &global_storage_path,
                )
                .await;
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: resolution_msg,
                    is_error: false,
                })
                .await;

            // ── Determine output file ────────────────────────────────────
            let output_dir_name = params
                .output_dir
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_str())
                .unwrap_or(&config.build.output_dir);
            let output_dir = root_path.join(output_dir_name);
            std::fs::create_dir_all(&output_dir).ok();

            let filename = story_title
                .as_deref()
                .map(sanitize_filename)
                .unwrap_or_else(|| "index".to_string());
            let output_file = output_dir.join(format!("{}.html", filename));

            return self
                .run_tweego(
                    &compiler_path,
                    &fallback,
                    &output_file,
                    &params,
                    &config,
                    &tweego_path_value,
                    &root_path,
                )
                .await;
        }

        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: format!("Knot: Compiling source from: {}", source_path.display()),
                is_error: false,
            })
            .await;

        // ── Resolve story formats ─────────────────────────────────────────
        //
        // Architecture: user setting → versioned managed cache → error
        //
        // The workspace is purely game files — story formats live in the
        // extension-managed folder (<globalStorage>/storyformats/). We set
        // TWEEGO_PATH to point tweego at the resolved formats directory.
        //
        // Resolution:
        //   a. If knot.build.storyformatsPath setting is set → set TWEEGO_PATH
        //   b. Else if <globalStorage>/storyformats/<id>@<ver>/ exists → set
        //      TWEEGO_PATH to the versioned cache dir
        //   c. Else → error with download hint
        let (resolution_msg, tweego_path_value) = self
            .resolve_story_formats(
                &params,
                &config,
                &story_format,
                &format_version,
                &global_storage_path,
            )
            .await;

        self.client
            .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                line: resolution_msg,
                is_error: false,
            })
            .await;

        // ── Determine output file ────────────────────────────────────────
        let output_dir_name = params
            .output_dir
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.as_str())
            .unwrap_or(&config.build.output_dir);
        let output_dir = root_path.join(output_dir_name);
        std::fs::create_dir_all(&output_dir).ok();

        // Derive filename from StoryTitle (sanitized), fallback to index.html.
        // This matches Twine GUI behavior where the compiled HTML is named
        // after the story title.
        let filename = story_title
            .as_deref()
            .map(sanitize_filename)
            .unwrap_or_else(|| "index".to_string());
        let output_file = output_dir.join(format!("{}.html", filename));

        self.run_tweego(
            &compiler_path,
            &source_path,
            &output_file,
            &params,
            &config,
            &tweego_path_value,
            &root_path,
        )
        .await
    }

    /// Resolve the story formats directory and build a diagnostic message.
    ///
    /// Returns `(log_message, optional_tweego_path)`. When `tweego_path` is
    /// `Some`, the caller sets it as the `TWEEGO_PATH` env var for the tweego
    /// process.
    async fn resolve_story_formats(
        &self,
        params: &KnotBuildParams,
        config: &knot_core::workspace::KnotConfig,
        story_format: &Option<knot_core::passage::StoryFormat>,
        format_version: &Option<String>,
        global_storage_path: &Option<std::path::PathBuf>,
    ) -> (String, Option<String>) {
        let user_storyformats = params
            .storyformats_path
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| config.storyformats_path.clone());

        // Versioned managed cache: <globalStorage>/storyformats/sugarcube-2@2.37.0/
        // Validate that format.js actually exists inside, not just that the
        // directory exists — a failed download can leave an empty directory.
        let versioned_managed = match (global_storage_path, story_format, format_version) {
            (Some(gs), Some(fmt), Some(ver)) => {
                let format_id = format_to_id(fmt);
                let versioned_dir = gs
                    .join("storyformats")
                    .join(format!("{}@{}", format_id, ver));
                let format_js = versioned_dir.join(format_id).join("format.js");
                if format_js.exists() {
                    Some(versioned_dir)
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(ref us) = user_storyformats {
            if us.is_dir() {
                return (
                    format!(
                        "Knot: Story formats: using configured path {}",
                        us.display()
                    ),
                    Some(us.to_string_lossy().to_string()),
                );
            } else {
                return (
                    format!(
                        "Knot: WARNING: Configured story formats path '{}' does not exist",
                        us.display()
                    ),
                    None,
                );
            }
        }

        if let Some(ref vm) = versioned_managed {
            return (
                format!(
                    "Knot: Story formats: using managed cache at {} (format={:?} version={})",
                    vm.display(),
                    story_format,
                    format_version.as_deref().unwrap_or("?")
                ),
                Some(vm.to_string_lossy().to_string()),
            );
        }

        // Nothing resolved — build will likely fail.
        let hint = match (story_format, format_version, global_storage_path) {
            (Some(fmt), Some(ver), Some(gs)) => {
                let format_id = format_to_id(fmt);
                let expected = gs
                    .join("storyformats")
                    .join(format!("{}@{}", format_id, ver))
                    .join(format_id)
                    .join("format.js");
                format!(
                    " — project needs {} v{} but it's not in the managed cache.\n\
                     Expected at: {}\n\
                     Use 'Knot: Configure Story Formats' to download it.",
                    format_id,
                    ver,
                    expected.display()
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
            _ => {
                " — no StoryData format/version detected. Is StoryData passage present?".to_string()
            }
        };

        (
            format!("Knot: No story formats directory resolved{}", hint),
            None,
        )
    }

    /// Execute tweego with the given parameters and stream output to the client.
    ///
    /// Merges `params.flags` (from VS Code settings) with `config.build.flags`
    /// (from `.vscode/knot.json`). Adds `-l` for stats logging. Parses the
    /// stats line from stdout and emits it as a build output line.
    #[allow(clippy::too_many_arguments)]
    async fn run_tweego(
        &self,
        compiler_path: &Path,
        source_path: &Path,
        output_file: &Path,
        params: &KnotBuildParams,
        config: &knot_core::workspace::KnotConfig,
        tweego_path_value: &Option<String>,
        root_path: &Path,
    ) -> Result<KnotBuildResponse, tower_lsp::jsonrpc::Error> {
        // Build the command arguments
        let mut args: Vec<String> = Vec::new();

        // If a start passage is specified, add --start flag
        if let Some(ref start_passage) = params.start_passage {
            args.push("--start".to_string());
            args.push(start_passage.clone());
        }

        // -l logs passage and word counts — always on, parsed from stdout
        // and emitted as a build stats line. Cheap, no downside.
        args.push("-l".to_string());

        args.push("-o".to_string());
        args.push(output_file.to_string_lossy().to_string());

        // Merge flags: VS Code setting flags + .vscode/knot.json flags.
        // Both sets apply — the setting is for common flags the user always
        // wants, knot.json is for project-specific flags.
        if let Some(ref flags) = params.flags {
            args.extend(flags.iter().cloned());
        }
        args.extend(config.build.flags.iter().cloned());

        // Source directory must be the LAST argument
        args.push(source_path.to_string_lossy().to_string());

        tracing::info!(
            "Build command: {} {}",
            compiler_path.display(),
            args.join(" ")
        );

        // Run the compiler with cwd set to the workspace root.
        let mut command = tokio::process::Command::new(compiler_path);
        command.args(&args).current_dir(root_path);

        if let Some(tp) = tweego_path_value {
            command.env("TWEEGO_PATH", tp);
            self.client
                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                    line: format!("Knot: Story formats search path = {}", tp),
                    is_error: false,
                })
                .await;
        }

        let output = command.output().await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Stream build output to the client, parsing for stats lines.
                // Tweego's -l flag emits lines like:
                //   Passages: 42 | Words: 12345
                // We extract and re-emit these as Knot stats lines.
                let mut stats_emitted = false;
                for line in stdout.lines() {
                    // Check for tweego's stats line format
                    if !stats_emitted
                        && (line.contains("Passages:") || line.contains("Words:"))
                        && (line.contains('|') || line.contains(','))
                    {
                        // Parse "Passages: N | Words: N" or similar
                        let passages = extract_stat(line, "Passages:");
                        let words = extract_stat(line, "Words:");
                        if let (Some(p), Some(w)) = (passages, words) {
                            self.client
                                .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                                    line: format!(
                                        "Knot: Build stats — {} passages, {} words",
                                        p, w
                                    ),
                                    is_error: false,
                                })
                                .await;
                            stats_emitted = true;
                            continue; // Don't emit the raw line too
                        }
                    }

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
        let build_result = self
            .knot_build(KnotBuildParams {
                workspace_uri: params.workspace_uri.clone(),
                start_passage: params.start_passage.clone(),
                compiler_path: params.compiler_path.clone(),
                source_dir: params.source_dir.clone(),
                output_dir: params.output_dir.clone(),
                storyformats_path: params.storyformats_path.clone(),
                managed_storyformats_path: params.managed_storyformats_path.clone(),
                flags: params.flags.clone(),
            })
            .await?;

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
                    params.workspace_uri,
                    root
                );
            }
        }

        let config = inner.workspace.config.clone();
        drop(inner);

        // Check configured path first
        if let Some(ref path) = config.compiler_path
            && path.exists()
        {
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
