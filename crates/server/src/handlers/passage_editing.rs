//! GUI passage editing requests — Tiers A, B, C of the Story Map editing
//! surface.
//!
//! The Story Map webview owns structure and metadata; passage *body* text
//! stays in the code editor (per the session's "no 1:1 equivalence"
//! scope). Every write here flows through the same battle-tested
//! pipeline as position saves: compute `TextEdit`s against
//! `open_documents` text under the write lock, release the lock, then
//! `client.apply_edit` — the LSP client applies the edits and
//! `did_change` re-syncs the server. No optimistic buffer mutation.
//!
//! - **Tier A** — `knot/updatePassageMetadata`: tags / color / group /
//!   size / manual `reachable` link / position on one passage.
//! - **Tier B** — `knot/createPassage`, `knot/deletePassage`,
//!   `knot/renamePassage` (rename also rewrites manual `reachable`
//!   declarations naming the old passage).
//! - **Tier C** — `knot/setStartPassage` (surgical StoryData `start`
//!   rewrite).

use crate::handlers::helpers;
use crate::handlers::navigation::ReferenceTarget;
use crate::lsp_ext::*;
use crate::state::ServerState;
use lsp_types::{TextEdit as LspTextEdit, WorkspaceEdit};
use std::collections::HashMap;
use url::Url;

impl ServerState {
    // -----------------------------------------------------------------------
    // Tier A — knot/updatePassageMetadata
    // -----------------------------------------------------------------------

    /// `knot/updatePassageMetadata` — edit one passage's header metadata
    /// from the Story Map inspector.
    ///
    /// Applies the three-state field semantics of
    /// [`helpers::HeaderEdits`] to the passage's header line and applies
    /// the single-line replacement through the standard edit pipeline.
    pub async fn knot_update_passage_metadata(
        &self,
        params: KnotUpdatePassageMetadataParams,
    ) -> Result<KnotUpdatePassageMetadataResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/updatePassageMetadata: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        // A declared manual link source must be a legal passage name (it
        // may not exist yet — that's flagged by diagnostics, not rejected
        // here — but it must at least survive a header round-trip).
        if let Some(Some(source)) = params.reachable.as_ref() {
            if let Err(e) = helpers::validate_passage_name(source) {
                return Ok(KnotUpdatePassageMetadataResponse {
                    success: false,
                    new_header: None,
                    error: Some(format!("Invalid manual link source: {}", e)),
                });
            }
        }

        let Some(file_uri) = inner.workspace.find_passage_file_uri(&params.passage_name) else {
            return Ok(KnotUpdatePassageMetadataResponse {
                success: false,
                new_header: None,
                error: Some(format!(
                    "Passage '{}' not found in workspace",
                    params.passage_name
                )),
            });
        };

        let Some(text) = inner.open_documents.get(&file_uri) else {
            return Ok(KnotUpdatePassageMetadataResponse {
                success: false,
                new_header: None,
                error: Some(format!(
                    "File text not available for passage '{}' ({})",
                    params.passage_name, file_uri
                )),
            });
        };

        // Single header-line pass — first match wins (the established
        // convention for duplicate passage names).
        let mut edit: Option<(u32, String)> = None;
        for (line_idx, line) in text.lines().enumerate() {
            if !line.starts_with("::") {
                continue;
            }
            let name = helpers::parse_passage_name_from_header(&line[2..]);
            if name == params.passage_name {
                let edits = helpers::HeaderEdits {
                    position: params.position.map(|p| (p.x, p.y)),
                    group: params.group.as_ref().map(|g| g.as_deref()),
                    color: params.color.as_ref().map(|c| c.as_deref()),
                    size: params.size.map(|s| s.map(|v| (v.width, v.height))),
                    reachable: params.reachable.as_ref().map(|r| r.as_deref()),
                    tags: params.tags.as_deref(),
                };
                match helpers::update_passage_header(line, edits) {
                    Ok(new_line) => {
                        edit = Some((line_idx as u32, new_line));
                    }
                    Err(e) => {
                        return Ok(KnotUpdatePassageMetadataResponse {
                            success: false,
                            new_header: None,
                            error: Some(e),
                        });
                    }
                }
                break;
            }
        }

        let Some((line_idx, new_line)) = edit else {
            return Ok(KnotUpdatePassageMetadataResponse {
                success: false,
                new_header: None,
                error: Some(format!(
                    "Could not find header line for passage '{}' in file {}",
                    params.passage_name, file_uri
                )),
            });
        };

        let old_line = text
            .lines()
            .nth(line_idx as usize)
            .unwrap_or_default()
            .to_string();

        let changes: HashMap<Url, Vec<LspTextEdit>> = HashMap::from([(
            file_uri.clone(),
            vec![LspTextEdit {
                range: lsp_types::Range {
                    start: lsp_types::Position {
                        line: line_idx,
                        character: 0,
                    },
                    end: lsp_types::Position {
                        line: line_idx,
                        character: helpers::utf16_len(&old_line),
                    },
                },
                new_text: new_line.clone(),
            }],
        )]);

        drop(inner);

        let workspace_edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        };
        if let Err(e) = self.client.apply_edit(workspace_edit).await {
            tracing::error!("knot/updatePassageMetadata: failed to apply edit: {}", e);
            return Ok(KnotUpdatePassageMetadataResponse {
                success: false,
                new_header: None,
                error: Some(format!("Failed to apply workspace edit: {}", e)),
            });
        }

        tracing::info!(
            "knot/updatePassageMetadata: updated '{}'",
            params.passage_name
        );

        Ok(KnotUpdatePassageMetadataResponse {
            success: true,
            new_header: Some(new_line),
            error: None,
        })
    }

    // -----------------------------------------------------------------------
    // knot/bulkUpdateMetadata — atomic multi-passage metadata editing
    // -----------------------------------------------------------------------

    /// `knot/bulkUpdateMetadata` — apply one metadata patch to many
    /// passages in a single atomic request.
    ///
    /// Used by the Story Map for group create/rename/dissolve and
    /// multi-select tag/color/group/size edits. Mirrors the edit-grouping
    /// strategy of `knot/updatePositions`: resolve each passage to its
    /// file, do ONE header-line pass per file, then apply everything as a
    /// single `WorkspaceEdit` — so bulk edits cause one refresh instead of
    /// N sequential round-trips. Field semantics are identical to
    /// `knot/updatePassageMetadata` (three-state patch).
    pub async fn knot_bulk_update_metadata(
        &self,
        params: KnotBulkUpdateMetadataParams,
    ) -> Result<KnotBulkUpdateMetadataResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/bulkUpdateMetadata: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        if params.passage_names.is_empty() {
            return Ok(KnotBulkUpdateMetadataResponse {
                success: false,
                updated_count: 0,
                errors: vec!["No passages listed for bulk update".to_string()],
            });
        }

        // Tags are validated once up-front — the header writer would
        // otherwise fail per-passage with the same error N times.
        if let Some(tags) = params.tags.as_deref() {
            for tag in tags {
                if let Err(e) = helpers::validate_tag(tag) {
                    return Ok(KnotBulkUpdateMetadataResponse {
                        success: false,
                        updated_count: 0,
                        errors: vec![e],
                    });
                }
            }
        }

        // Resolve each passage to its file, then group by file.
        let mut updates_by_file: HashMap<Url, Vec<String>> = HashMap::new();
        let mut errors: Vec<String> = Vec::new();
        for name in &params.passage_names {
            match inner.workspace.find_passage_file_uri(name) {
                Some(file_uri) => updates_by_file
                    .entry(file_uri)
                    .or_default()
                    .push(name.clone()),
                None => errors.push(format!("Passage '{}' not found in workspace", name)),
            }
        }

        let mut changes: HashMap<Url, Vec<LspTextEdit>> = HashMap::new();
        let mut updated_count: u32 = 0;

        for (file_uri, names) in &updates_by_file {
            let Some(text) = inner.open_documents.get(file_uri) else {
                for name in names {
                    errors.push(format!(
                        "File text not available for passage '{}' ({})",
                        name, file_uri
                    ));
                }
                continue;
            };

            let pending: std::collections::HashSet<&str> =
                names.iter().map(|s| s.as_str()).collect();
            let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Single header-line pass per file; first header per name wins
            // (the established duplicate-name convention).
            for (line_idx, line) in text.lines().enumerate() {
                if !line.starts_with("::") {
                    continue;
                }
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if !pending.contains(name.as_str()) || !found.insert(name.clone()) {
                    continue;
                }

                let edits = helpers::HeaderEdits {
                    position: None,
                    group: params.group.as_ref().map(|g| g.as_deref()),
                    color: params.color.as_ref().map(|c| c.as_deref()),
                    size: params
                        .size
                        .as_ref()
                        .map(|s| s.as_ref().map(|v| (v.width, v.height))),
                    reachable: None,
                    tags: params.tags.as_deref(),
                };
                match helpers::update_passage_header(line, edits) {
                    Ok(new_line) => {
                        changes.entry(file_uri.clone()).or_default().push(LspTextEdit {
                            range: lsp_types::Range {
                                start: lsp_types::Position {
                                    line: line_idx as u32,
                                    character: 0,
                                },
                                end: lsp_types::Position {
                                    line: line_idx as u32,
                                    character: helpers::utf16_len(line),
                                },
                            },
                            new_text: new_line,
                        });
                        updated_count += 1;
                    }
                    Err(e) => {
                        errors.push(format!("Passage '{}': {}", name, e));
                    }
                }
            }

            for name in names {
                if !found.contains(name) {
                    errors.push(format!(
                        "Could not find header line for passage '{}' in file {}",
                        name, file_uri
                    ));
                }
            }
        }

        drop(inner);

        if !changes.is_empty() {
            let workspace_edit = WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            };
            if let Err(e) = self.client.apply_edit(workspace_edit).await {
                tracing::error!("knot/bulkUpdateMetadata: failed to apply edit: {}", e);
                return Ok(KnotBulkUpdateMetadataResponse {
                    success: false,
                    updated_count: 0,
                    errors: vec![format!("Failed to apply workspace edit: {}", e)],
                });
            }
        }

        tracing::info!(
            "knot/bulkUpdateMetadata: patched {} passages ({} errors)",
            updated_count,
            errors.len()
        );

        Ok(KnotBulkUpdateMetadataResponse {
            success: errors.is_empty(),
            updated_count,
            errors,
        })
    }

    // -----------------------------------------------------------------------
    // Tier B — knot/createPassage
    // -----------------------------------------------------------------------

    /// `knot/createPassage` — append a new passage to a twee file.
    ///
    /// Target-file policy when the request doesn't name one: the file
    /// containing StoryData, else the file with the most passages, else
    /// any open document. The passage text carries the creation tags and
    /// the graph-canvas position so the node appears exactly where the
    /// author created it.
    pub async fn knot_create_passage(
        &self,
        params: KnotCreatePassageParams,
    ) -> Result<KnotCreatePassageResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/createPassage: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        if let Err(e) = helpers::validate_passage_name(&params.name) {
            return Ok(KnotCreatePassageResponse {
                success: false,
                file: None,
                line: None,
                error: Some(e),
            });
        }
        if inner.workspace.find_passage_file_uri(&params.name).is_some() {
            return Ok(KnotCreatePassageResponse {
                success: false,
                file: None,
                line: None,
                error: Some(format!(
                    "A passage named '{}' already exists in the workspace",
                    params.name
                )),
            });
        }
        if let Some(tags) = params.tags.as_deref() {
            for tag in tags {
                if let Err(e) = helpers::validate_tag(tag) {
                    return Ok(KnotCreatePassageResponse {
                        success: false,
                        file: None,
                        line: None,
                        error: Some(e),
                    });
                }
            }
        }

        // ── Target file resolution ─────────────────────────────────────
        // `target_file` (a file URI) wins when present. Three cases:
        //   1. The file is in the text cache → normal append path.
        //   2. The file exists on disk but isn't cached (watcher missed it,
        //      e.g. just created) → read from disk and append.
        //   3. `create_file` is set and the file doesn't exist → create it
        //      atomically with the passage via a `CreateFile` document
        //      change ahead of the `Edit` (one WorkspaceEdit — no
        //      half-created files on failure).
        let (target_uri, file_text, needs_create): (Option<Url>, Option<String>, bool) =
            match params.target_file.as_deref() {
                Some(f) => {
                    let Ok(uri) = f.parse::<Url>() else {
                        return Ok(KnotCreatePassageResponse {
                            success: false,
                            file: None,
                            line: None,
                            error: Some(format!("Invalid target file URI: {}", f)),
                        });
                    };
                    if let Some(text) = inner.open_documents.get(&uri).cloned() {
                        (Some(uri), Some(text), false)
                    } else if let Ok(path) = uri.to_file_path()
                        && path.exists()
                        && let Ok(text) = std::fs::read_to_string(&path)
                    {
                        (Some(uri), Some(text), false)
                    } else if params.create_file.unwrap_or(false) {
                        (Some(uri), None, true)
                    } else {
                        return Ok(KnotCreatePassageResponse {
                            success: false,
                            file: None,
                            line: None,
                            error: Some(format!("Target file not found: {}", f)),
                        });
                    }
                }
                None => {
                    let uri = inner
                        .workspace
                        .find_passage_file_uri("StoryData")
                        .or_else(|| {
                            inner
                                .workspace
                                .documents()
                                .max_by_key(|doc| doc.passages.len())
                                .map(|doc| doc.uri.clone())
                        });
                    match uri {
                        Some(u) => {
                            let text = inner.open_documents.get(&u).cloned();
                            (Some(u), text, false)
                        }
                        None => (None, None, false),
                    }
                }
            };

        let Some(file_uri) = target_uri else {
            return Ok(KnotCreatePassageResponse {
                success: false,
                file: None,
                line: None,
                error: Some(
                    "No twee files in the workspace — create a file first".to_string(),
                ),
            });
        };

        // ── New-file validation ────────────────────────────────────────
        // The path must stay inside the workspace root and be a twee file.
        if needs_create {
            let root = inner.workspace.root_uri.to_file_path().ok();
            let target_path = file_uri.to_file_path().ok();
            let (Some(root), Some(target)) = (root, target_path) else {
                return Ok(KnotCreatePassageResponse {
                    success: false,
                    file: None,
                    line: None,
                    error: Some("Cannot create a file outside the workspace".to_string()),
                });
            };
            let ext_ok = target
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e == "twee" || e == "tw");
            if !ext_ok {
                return Ok(KnotCreatePassageResponse {
                    success: false,
                    file: None,
                    line: None,
                    error: Some(
                        "New file must end in '.twee' or '.tw'".to_string(),
                    ),
                });
            }
            if !target.starts_with(&root) {
                return Ok(KnotCreatePassageResponse {
                    success: false,
                    file: None,
                    line: None,
                    error: Some(
                        "New file must be inside the workspace root".to_string(),
                    ),
                });
            }
        }

        let text = file_text.unwrap_or_default();

        // ── Build the passage text ─────────────────────────────────────
        let mut header = format!(":: {}", params.name);
        if let Some(tags) = params.tags.as_deref()
            && !tags.is_empty()
        {
            header.push_str(&format!(" [{}]", tags.join(" ")));
        }
        if let Some(pos) = params.position.as_ref() {
            let edits = helpers::HeaderEdits {
                position: Some((pos.x, pos.y)),
                ..Default::default()
            };
            // Can't fail on a line we just built with no tags/JSON.
            if let Ok(with_pos) = helpers::update_passage_header(&header, edits) {
                header = with_pos;
            }
        }

        let line_count = text.lines().count() as u32;
        let (insert_line, new_text, header_line) = if text.trim().is_empty() {
            (0u32, format!("{}\n", header), 0u32)
        } else if text.ends_with('\n') {
            (line_count, format!("\n{}\n", header), line_count + 1)
        } else {
            (line_count, format!("\n{}\n", header), line_count)
        };

        let insert_edit = LspTextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: insert_line,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: insert_line,
                    character: 0,
                },
            },
            new_text,
        };

        drop(inner);

        // New-file creation goes through `document_changes` so the file is
        // created and the passage appended in ONE atomic WorkspaceEdit —
        // a failure can never leave an empty half-created file behind.
        // The client's file watcher then reports the new file to us via
        // didChangeWatchedFiles(CREATED), which indexes it into the model.
        let workspace_edit = if needs_create {
            use lsp_types::{
                CreateFile, CreateFileOptions, DocumentChangeOperation, DocumentChanges,
                ResourceOp, TextDocumentEdit, OptionalVersionedTextDocumentIdentifier,
            };
            WorkspaceEdit {
                changes: None,
                document_changes: Some(DocumentChanges::Operations(vec![
                    DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                        uri: file_uri.clone(),
                        options: Some(CreateFileOptions {
                            overwrite: Some(false),
                            ignore_if_exists: Some(true),
                        }),
                        annotation_id: None,
                    })),
                    DocumentChangeOperation::Edit(TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: file_uri.clone(),
                            version: None,
                        },
                        edits: vec![lsp_types::OneOf::Left(insert_edit)],
                    }),
                ])),
                change_annotations: None,
            }
        } else {
            let changes: HashMap<Url, Vec<LspTextEdit>> = HashMap::from([(
                file_uri.clone(),
                vec![insert_edit],
            )]);
            WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }
        };

        if let Err(e) = self.client.apply_edit(workspace_edit).await {
            tracing::error!("knot/createPassage: failed to apply edit: {}", e);
            return Ok(KnotCreatePassageResponse {
                success: false,
                file: None,
                line: None,
                error: Some(format!("Failed to apply workspace edit: {}", e)),
            });
        }

        tracing::info!(
            "knot/createPassage: created '{}' in {}{}",
            params.name,
            file_uri,
            if needs_create { " (new file)" } else { "" }
        );

        Ok(KnotCreatePassageResponse {
            success: true,
            file: Some(file_uri.to_string()),
            line: Some(header_line),
            error: None,
        })
    }

    // -----------------------------------------------------------------------
    // Tier B — knot/deletePassage
    // -----------------------------------------------------------------------

    /// `knot/deletePassage` — remove a passage's whole block (header +
    /// body) and optionally unwrap wikilinks pointing at it.
    ///
    /// `strip_links` follows Twine's "also remove links to this passage"
    /// behavior, scoped to `[[...]]` wikilinks only: macro-based links
    /// (`<<goto>>`, `<<link>>`) have paired closing tags and body-span
    /// structure that make blind text substitution unsafe, so they are
    /// left to become broken links (the existing quickfixes handle
    /// them).
    pub async fn knot_delete_passage(
        &self,
        params: KnotDeletePassageParams,
    ) -> Result<KnotDeletePassageResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/deletePassage: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        let Some(file_uri) = inner.workspace.find_passage_file_uri(&params.passage_name) else {
            return Ok(KnotDeletePassageResponse {
                success: false,
                links_stripped: 0,
                error: Some(format!(
                    "Passage '{}' not found in workspace",
                    params.passage_name
                )),
            });
        };

        let Some(text) = inner.open_documents.get(&file_uri).cloned() else {
            return Ok(KnotDeletePassageResponse {
                success: false,
                links_stripped: 0,
                error: Some(format!(
                    "File text not available for passage '{}' ({})",
                    params.passage_name, file_uri
                )),
            });
        };

        // ── Locate the passage block: header line → next `::` header ──
        let mut header_line: Option<u32> = None;
        let mut end_line: u32 = text.lines().count() as u32;
        let mut seen_header = false;
        for (i, line) in text.lines().enumerate() {
            if !line.starts_with("::") {
                continue;
            }
            if !seen_header {
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if name == params.passage_name {
                    header_line = Some(i as u32);
                    seen_header = true;
                }
            } else {
                end_line = i as u32;
                break;
            }
        }

        let Some(header_line) = header_line else {
            return Ok(KnotDeletePassageResponse {
                success: false,
                links_stripped: 0,
                error: Some(format!(
                    "Could not find header line for passage '{}' in file {}",
                    params.passage_name, file_uri
                )),
            });
        };

        let mut changes: HashMap<Url, Vec<LspTextEdit>> = HashMap::new();

        // Delete the whole block: [header start, next header start) —
        // this consumes the blank separator lines before the next
        // passage, leaving a clean boundary.
        changes.entry(file_uri.clone()).or_default().push(LspTextEdit {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: header_line,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: end_line,
                    character: 0,
                },
            },
            new_text: String::new(),
        });

        // ── Optional link stripping (wikilinks only) ───────────────────
        let mut links_stripped: u32 = 0;
        if params.strip_links {
            for doc in inner.workspace.documents() {
                let Some(doc_text) = inner.open_documents.get(&doc.uri) else {
                    continue;
                };
                let mut doc_edits: Vec<LspTextEdit> = Vec::new();
                for passage in &doc.passages {
                    // Skip the passage being deleted (its links vanish
                    // with the block).
                    if passage.name == params.passage_name {
                        continue;
                    }
                    for link in &passage.links {
                        if link.target.trim() != params.passage_name {
                            continue;
                        }
                        let abs_span = passage.abs_range(&link.span);
                        // Only unwrap plain `[[...]]` wikilinks — the
                        // macro forms carry markup that blind span
                        // replacement would break.
                        let link_text = doc_text
                            .get(abs_span.clone())
                            .unwrap_or_default();
                        if !link_text.starts_with("[[") {
                            continue;
                        }
                        let replacement = link
                            .display_text
                            .clone()
                            .unwrap_or_else(|| link.target.clone());
                        doc_edits.push(LspTextEdit {
                            range: helpers::byte_range_to_lsp_range(doc_text, &abs_span),
                            new_text: replacement,
                        });
                        links_stripped += 1;
                    }
                }
                if !doc_edits.is_empty() {
                    changes
                        .entry(doc.uri.clone())
                        .or_default()
                        .extend(doc_edits);
                }
            }
        }

        drop(inner);

        let workspace_edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        };
        if let Err(e) = self.client.apply_edit(workspace_edit).await {
            tracing::error!("knot/deletePassage: failed to apply edit: {}", e);
            return Ok(KnotDeletePassageResponse {
                success: false,
                links_stripped: 0,
                error: Some(format!("Failed to apply workspace edit: {}", e)),
            });
        }

        tracing::info!(
            "knot/deletePassage: deleted '{}' ({} links stripped)",
            params.passage_name,
            links_stripped
        );

        Ok(KnotDeletePassageResponse {
            success: true,
            links_stripped,
            error: None,
        })
    }

    // -----------------------------------------------------------------------
    // Tier B — knot/renamePassage
    // -----------------------------------------------------------------------

    /// `knot/renamePassage` — rename a passage across the workspace
    /// through the same machinery as the editor's F2 rename (header +
    /// all link targets, cross-file), plus rewriting any manual
    /// `reachable` metadata declarations that name the old passage.
    pub async fn knot_rename_passage(
        &self,
        params: KnotRenamePassageParams,
    ) -> Result<KnotRenamePassageResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/renamePassage: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        if let Err(e) = helpers::validate_passage_name(&params.new_name) {
            return Ok(KnotRenamePassageResponse {
                success: false,
                files_changed: 0,
                error: Some(e),
            });
        }
        if inner
            .workspace
            .find_passage_file_uri(&params.new_name)
            .is_some()
        {
            return Ok(KnotRenamePassageResponse {
                success: false,
                files_changed: 0,
                error: Some(format!(
                    "A passage named '{}' already exists in the workspace",
                    params.new_name
                )),
            });
        }

        let target = ReferenceTarget::Passage {
            name: params.name.clone(),
        };

        // Same failsafe as the LSP rename: never produce call-site edits
        // while the definition itself can't be resolved.
        if !crate::handlers::navigation::definition_confirmed(&target, &inner) {
            return Ok(KnotRenamePassageResponse {
                success: false,
                files_changed: 0,
                error: Some(format!(
                    "Passage '{}' not found in workspace",
                    params.name
                )),
            });
        }

        // collect_rename_edits already includes the manual `reachable`
        // metadata rewrite (see navigation.rs).
        let changes =
            crate::handlers::navigation::collect_rename_edits(&target, &params.new_name, &inner);

        if changes.is_empty() {
            return Ok(KnotRenamePassageResponse {
                success: false,
                files_changed: 0,
                error: Some(format!(
                    "No editable sites found for passage '{}'",
                    params.name
                )),
            });
        }

        let files_changed = changes.len() as u32;

        drop(inner);

        let workspace_edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        };
        if let Err(e) = self.client.apply_edit(workspace_edit).await {
            tracing::error!("knot/renamePassage: failed to apply edit: {}", e);
            return Ok(KnotRenamePassageResponse {
                success: false,
                files_changed: 0,
                error: Some(format!("Failed to apply workspace edit: {}", e)),
            });
        }

        tracing::info!(
            "knot/renamePassage: renamed '{}' → '{}' across {} files",
            params.name,
            params.new_name,
            files_changed
        );

        Ok(KnotRenamePassageResponse {
            success: true,
            files_changed,
            error: None,
        })
    }

    // -----------------------------------------------------------------------
    // Tier C — knot/setStartPassage
    // -----------------------------------------------------------------------

    /// `knot/setStartPassage` — rewrite the `start` field of the
    /// StoryData passage's JSON body, surgically (preserving the body's
    /// formatting), and update the workspace's derived metadata so the
    /// graph re-roots immediately.
    pub async fn knot_set_start_passage(
        &self,
        params: KnotSetStartPassageParams,
    ) -> Result<KnotSetStartPassageResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.write().await;

        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/setStartPassage: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri,
                    root
                );
            }
        }

        if inner
            .workspace
            .find_passage(&params.passage_name)
            .is_none()
        {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some(format!(
                    "Passage '{}' not found in workspace",
                    params.passage_name
                )),
            });
        }

        let Some((doc, _story_data)) = inner.workspace.find_passage("StoryData") else {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some(
                    "No StoryData passage found in the workspace — add one with a `start` field first"
                        .to_string(),
                ),
            });
        };

        let Some(text) = inner.open_documents.get(&doc.uri).cloned() else {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some(format!(
                    "File text not available for StoryData ({})",
                    doc.uri
                )),
            });
        };

        // ── Locate the StoryData body region (line range) ──────────────
        let mut body_start: Option<u32> = None;
        let mut body_end: u32 = text.lines().count() as u32;
        let mut in_story_data = false;
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                if in_story_data {
                    body_end = i as u32;
                    break;
                }
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if name == "StoryData" {
                    in_story_data = true;
                    body_start = Some(i as u32 + 1);
                }
            }
        }

        let Some(body_start) = body_start else {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some("StoryData passage has no body".to_string()),
            });
        };

        let body_text: String = text
            .split('\n') // keeps \r for CRLF files — offsets must match the raw text
            .skip(body_start as usize)
            .take((body_end - body_start) as usize)
            .collect::<Vec<_>>()
            .join("\n");

        // Validate the body parses as JSON before touching it.
        if serde_json::from_str::<serde_json::Value>(body_text.trim()).is_err() {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some("StoryData body is not valid JSON".to_string()),
            });
        }

        // ── Compute the surgical edit inside the body ──────────────────
        let edit = compute_storydata_start_edit(&body_text, &params.passage_name);

        let Some((rel_range, new_text)) = edit else {
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some(
                    "Could not locate the `start` field in StoryData".to_string(),
                ),
            });
        };

        // The body starts at the beginning of line `body_start`; convert
        // the body-relative byte range to a document LSP range by prefixing
        // the exact byte length of the preceding lines (split on '\n' so
        // CRLF line endings keep their '\r' bytes in the count).
        let body_offset: usize = text
            .split('\n')
            .take(body_start as usize)
            .map(|l| l.len() + 1)
            .sum();
        let abs_start = body_offset + rel_range.start;
        let abs_end = body_offset + rel_range.end;
        let range = helpers::byte_range_to_lsp_range(&text, &(abs_start..abs_end));

        let changes: HashMap<Url, Vec<LspTextEdit>> = HashMap::from([(
            doc.uri.clone(),
            vec![LspTextEdit {
                range,
                new_text,
            }],
        )]);

        drop(inner);

        let workspace_edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        };
        if let Err(e) = self.client.apply_edit(workspace_edit).await {
            tracing::error!("knot/setStartPassage: failed to apply edit: {}", e);
            return Ok(KnotSetStartPassageResponse {
                success: false,
                error: Some(format!("Failed to apply workspace edit: {}", e)),
            });
        }

        // ── Sync derived workspace state ───────────────────────────────
        // StoryData metadata is extracted by the indexing pipeline only
        // (apply_document_update deliberately doesn't re-extract it), so
        // the new start is recorded here. The Startup → Start upstream
        // bridge is rebuilt to point at the new start.
        {
            let mut inner = self.inner.write().await;
            if let Some(metadata) = inner.workspace.metadata.as_mut() {
                metadata.start_passage = params.passage_name.clone();
            }
            inner
                .workspace
                .graph
                .remove_edges_of_type(knot_core::graph::EdgeType::Upstream);
            inner.workspace.rebuild_upstream_edges();
        }

        tracing::info!(
            "knot/setStartPassage: start passage set to '{}'",
            params.passage_name
        );

        Ok(KnotSetStartPassageResponse {
            success: true,
            error: None,
        })
    }
}

/// Compute the surgical text edit that writes `"start": "<name>"` into a
/// StoryData JSON body.
///
/// Returns `(body-relative byte range, replacement text)`:
/// - When a `"start"` key exists, only its **string value** is replaced,
///   preserving all surrounding formatting.
/// - When the key is absent, `"start": "<name>",` is inserted directly
///   after the opening `{` (with a newline + matched indentation for
///   pretty-printed bodies).
///
/// Returns `None` when the value exists but isn't a string (the
/// surgical scanner can't safely rewrite other shapes).
fn compute_storydata_start_edit(body: &str, name: &str) -> Option<(std::ops::Range<usize>, String)> {
    let quoted = serde_json::to_string(name).unwrap_or_else(|_| format!("\"{}\"", name));

    // Find the `"start"` key: a quoted "start" followed by optional
    // whitespace and a colon.
    let bytes = body.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel) = body[search_from..].find("\"start\"") {
        let key_at = search_from + rel;
        // Ensure this is a standalone key (not part of a longer name like
        // "start-extra" — the next char after the closing quote must be
        // whitespace or ':').
        let mut after = key_at + "\"start\"".len();
        while after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'\t' || bytes[after] == b'\n' || bytes[after] == b'\r') {
            after += 1;
        }
        if after < bytes.len() && bytes[after] == b':' {
            // Value starts after the colon.
            let mut value_at = after + 1;
            while value_at < bytes.len()
                && (bytes[value_at] == b' ' || bytes[value_at] == b'\t')
            {
                value_at += 1;
            }
            if value_at < bytes.len() && bytes[value_at] == b'"' {
                // Find the closing quote (JSON strings in StoryData don't
                // contain escapes in practice, but handle \" anyway).
                let mut close = value_at + 1;
                while close < bytes.len() {
                    if bytes[close] == b'\\' {
                        close += 2;
                        continue;
                    }
                    if bytes[close] == b'"' {
                        break;
                    }
                    close += 1;
                }
                if close < bytes.len() {
                    return Some((value_at..close + 1, quoted));
                }
            }
            // Non-string value — not safe to rewrite surgically.
            return None;
        }
        search_from = key_at + "\"start\"".len();
    }

    // No `"start"` key — insert one right after the opening `{`.
    let brace = body.find('{')?;
    let after_brace = brace + 1;

    // Multi-line pretty JSON: insert on its own line with the indentation
    // of the first content line. Single-line: insert inline.
    let rest = &body[after_brace..];
    let multiline = rest.starts_with('\n') || rest.starts_with("\r\n");
    if multiline {
        // Indentation of the first non-blank content line (skipping the
        // empty segment before the leading newline).
        let first_content = rest
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let indent_len = first_content.len() - first_content.trim_start().len();
        let indent = &first_content[..indent_len];
        Some((
            after_brace..after_brace,
            format!("\n{}\"start\": {},", indent, quoted),
        ))
    } else {
        Some((
            after_brace..after_brace,
            format!("\"start\": {}, ", quoted),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::compute_storydata_start_edit;

    #[test]
    fn replaces_existing_string_value_only() {
        let body = "{\n  \"ifid\": \"ABC\",\n  \"start\": \"Start\",\n  \"format\": \"SugarCube\"\n}\n";
        let (range, text) =
            compute_storydata_start_edit(body, "Prologue").expect("edit should compute");
        assert_eq!(text, "\"Prologue\"");
        assert_eq!(&body[range.clone()], "\"Start\"");
    }

    #[test]
    fn inserts_into_pretty_body_without_start() {
        let body = "{\n    \"ifid\": \"ABC\",\n    \"format\": \"SugarCube\"\n}\n";
        let (range, text) =
            compute_storydata_start_edit(body, "Prologue").expect("edit should compute");
        // Inserted right after `{`, on its own line with matched indent.
        assert_eq!(text, "\n    \"start\": \"Prologue\",");
        assert!(range.is_empty(), "insert ranges are empty");
        let result = format!(
            "{}{}{}",
            &body[..range.start],
            text,
            &body[range.end..]
        );
        assert!(result.contains("\"start\": \"Prologue\","), "{}", result);
        assert!(result.contains("\"ifid\": \"ABC\","), "{}", result);
    }

    #[test]
    fn inserts_into_single_line_body() {
        let body = "{\"ifid\":\"ABC\"}";
        let (range, text) =
            compute_storydata_start_edit(body, "Prologue").expect("edit should compute");
        assert_eq!(text, "\"start\": \"Prologue\", ");
        let result = format!(
            "{}{}{}",
            &body[..range.start],
            text,
            &body[range.end..]
        );
        assert_eq!(result, "{\"start\": \"Prologue\", \"ifid\":\"ABC\"}");
    }

    #[test]
    fn ignores_longer_keys_named_start_something() {
        let body = "{\n  \"start-extra\": \"nope\",\n  \"start\": \"Start\"\n}\n";
        let (range, text) =
            compute_storydata_start_edit(body, "Prologue").expect("edit should compute");
        assert_eq!(text, "\"Prologue\"");
        assert_eq!(&body[range.clone()], "\"Start\"");
    }

    #[test]
    fn rejects_non_string_value() {
        let body = "{\"start\": 42}";
        assert!(compute_storydata_start_edit(body, "X").is_none());
    }
}
