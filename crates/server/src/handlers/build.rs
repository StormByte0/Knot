//! Custom LSP request handlers for the build pipeline (knot/build, knot/play, knot/compilerDetect).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;

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

        // Resolve compiler path:
        // 1. Extension-provided override (from VS Code setting `knot.tweegoPath`)
        // 2. Config override from `.vscode/knot.json`
        // 3. PATH lookup
        let compiler_path = if let Some(ref ext_path) = params.compiler_path {
            Some(std::path::PathBuf::from(ext_path))
        } else if let Some(ref path) = config.compiler_path {
            Some(path.clone())
        } else {
            helpers::which_compiler()
        };

        let Some(compiler_path) = compiler_path else {
            return Ok(KnotBuildResponse {
                success: false,
                output_path: None,
                errors: vec![
                    "No Twine compiler found. Install Tweego and ensure it is on PATH, or set compiler_path in .vscode/knot.json".to_string()
                ],
            });
        };

        // Determine output directory
        let output_dir = root_path.join(&config.build.output_dir);
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
        args.push(root_path.to_string_lossy().to_string());

        tracing::info!("Build command: {} {}", compiler_path.display(), args.join(" "));

        // Run the compiler with cwd set to the workspace root.
        // Tweego automatically searches for `.storyformats` in: cwd,
        // the binary's directory, and system paths. Since `.storyformats`
        // typically lives at the project root, setting cwd to the workspace
        // root is sufficient — no explicit flag needed.
        let output = tokio::process::Command::new(&compiler_path)
            .args(&args)
            .current_dir(&root_path)
            .output()
            .await;

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
