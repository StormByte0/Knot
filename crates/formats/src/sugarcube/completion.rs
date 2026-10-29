//! Moved verbatim from the former `sugarcube/mod.rs` monolith
//! (pure move, plan §4.6 — no signature or logic changes).

use super::*;

// ===========================================================================
// Private completion builders (SugarCube-specific)
// ===========================================================================

/// Different contexts where passage names are offered as completions.
#[derive(Clone)]
pub(super) enum PassageCompletionKind {
    /// Inside a `[[link]]` — inserts `[[name]]`
    Link,
    /// Inside a macro passage-arg — inserts just the name.
    ///
    /// Carries the macro context so the builder can produce context-aware
    /// detail text (e.g., "Navigation target for <<goto>>" vs
    /// "Included passage for <<include>>") and proper text_edit ranges.
    MacroArg {
        /// Which macro this passage-ref arg belongs to (e.g., "goto", "link", "include").
        macro_name: String,
        /// Whether the macro invocation has a body block.
        has_body: bool,
    },
}

/// Compute a `FormatTextEdit` for macro completions.
///
/// ## Design: why textEdit does NOT cover `<<`
///
/// VS Code uses the text inside a completion's `textEdit` range as the
/// "prefix" for client-side filtering. If the range covers `<<`, the prefix
/// would be `<<` (or `<<li`), and VS Code would filter out items whose
/// `filter_text` doesn't start with `<<`. Since our `filter_text` values are
/// bare macro names (`"if"`, `"set"`, `"link"`), ALL items would be removed.
///
/// The fix: the textEdit range covers ONLY the partial name the user typed
/// AFTER `<<`. The `<<` itself remains in the document and the snippet is
/// inserted after it. This works because:
///
/// 1. **Empty prefix** (user typed `<<`): textEdit range starts at the
///    cursor (after `<<`) and extends to cover any auto-closed `>>`.
///    `new_text` is the snippet WITHOUT `<<`. The `<<` already in the
///    document + snippet = `<<macro ...>>`. VS Code's word at cursor is
///    empty → all items pass the filter.
///
/// 2. **Partial prefix** (user typed `<<li`): textEdit range covers just
///    `li` (from after `<<` to cursor), plus any auto-closed `>>`.
///    `new_text` is the snippet WITHOUT `<<`. The `<<` remains, `li` and
///    `>>` are replaced by the full snippet. VS Code's prefix is `li` →
///    matches `filter_text = "link"`.
///
/// ## Auto-close `>>` handling
///
/// When the user types `<<`, VS Code's auto-close pair feature may add `>>`
/// immediately after the cursor. If we don't consume this `>>`, accepting a
/// completion would leave a dangling `>>` at the end. We detect this by
/// checking if `>>` follows the cursor position and including it in the
/// textEdit range so it gets replaced.
fn compute_macro_text_edit(
    filter_prefix: &str,
    cursor: CursorPos,
    snippet: &str,
    after_cursor: &str,
) -> Option<crate::types::FormatTextEdit> {
    use crate::types::FormatTextEdit;

    // The snippet does NOT include `<<` — it starts with the macro name.
    // The `<<` is already in the document and will remain there.
    // The snippet IS self-contained with `>>` (and closing tags for block
    // macros). If VS Code auto-close also inserted `>>`, we consume it by
    // extending the replacement range so there's no duplication.
    let new_text = snippet.to_string();

    // Check if auto-close added `>>` after the cursor — if so, we need to
    // include it in the replacement range so it gets consumed.
    let auto_close_len = if after_cursor.starts_with(">>") {
        2u32
    } else {
        0u32
    };

    if !filter_prefix.is_empty() {
        // User typed `<<li` — replace just the partial name `li`
        let prefix_len = filter_prefix.chars().count() as u32;
        let start_char = cursor.character.saturating_sub(prefix_len);
        let end_char = cursor.character + auto_close_len;
        Some(FormatTextEdit {
            start_line: cursor.line,
            start_character: start_char,
            end_line: cursor.line,
            end_character: end_char,
            new_text,
        })
    } else {
        // User typed `<<` — insert snippet at cursor (possibly replacing auto-close `>>`)
        Some(FormatTextEdit {
            start_line: cursor.line,
            start_character: cursor.character,
            end_line: cursor.line,
            end_character: cursor.character + auto_close_len,
            new_text,
        })
    }
}

impl SugarCubePlugin {
    /// Build variable name completions, enriched with structural data
    /// from the arena tree.
    ///
    /// Since `$` is shorthand for `State.variables.` and `_` for
    /// `State.temporary.`, this is really "show children of the scope
    /// root" — the same operation as dot-notation property completions,
    /// just starting from a different depth in the arena tree.
    ///
    /// Each completion item carries the variable's inferred kind
    /// (Object/Array/Scalar/Unknown) from the arena `NavIndex`, which
    /// controls the icon, detail text, sort priority, and whether
    /// the `.` commit character is offered for chaining.
    pub(super) fn build_variable_completions(
        &self,
        is_temp: bool,
        line: u32,
        character: u32,
        partial: &str,
        passage_name: Option<&str>,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, PropertyKind,
        };

        // Use the passage-scoped accessor so SugarCube `_` temps from
        // other passages don't bleed into completions. Persistent (`$`)
        // vars are always returned (workspace-global by design); temps
        // are restricted to the enclosing passage. When `passage_name`
        // is `None` (cursor in pre-story preamble), only persistent
        // vars are returned — safe degradation, never leaks.
        let all_names = self.registry.variable_names_for_passage(passage_name);
        let mut sorted_names: Vec<_> = all_names.into_iter().collect();
        sorted_names.sort();

        sorted_names
            .iter()
            .filter(|name| {
                if is_temp {
                    name.starts_with('_') && !name.starts_with("__")
                } else {
                    !name.starts_with('_')
                }
            })
            .map(|name| {
                let display_name = if name.starts_with('$') || name.starts_with('_') {
                    name.clone()
                } else if is_temp {
                    format!("_{name}")
                } else {
                    format!("${name}")
                };
                let filter_name = name
                    .trim_start_matches('$')
                    .trim_start_matches('_')
                    .to_string();

                // ── Arena tree enrichment ──────────────────────────────
                // Look up the inferred structural kind and children for
                // this variable. `$` = children of <persistent> root,
                // `_` = children of <temp:Passage> root — the arena
                // path_index unifies both under the display name.
                //
                // Use the passage-aware variants so temp var paths
                // (`_x`) resolve against the enclosing passage's
                // namespaced path_index key. Persistent paths (`$x`)
                // behave identically to the non-passage-aware variants.
                let inferred_kind = self
                    .variable_kind_at_path_for_passage(&display_name, passage_name)
                    .unwrap_or(PropertyKind::Unknown);
                let children =
                    self.variable_children_with_kind_for_passage(&display_name, passage_name);
                let child_count = children.len();

                // Completion icon: Objects and Arrays use Module (they
                // have children to explore), Scalars use Variable.
                let completion_kind = match inferred_kind {
                    PropertyKind::Object | PropertyKind::Array => FormatCompletionKind::Module,
                    PropertyKind::Scalar | PropertyKind::Unknown => FormatCompletionKind::Variable,
                };

                // Detail: type-aware with child preview
                let detail = match inferred_kind {
                    PropertyKind::Object => {
                        let preview: Vec<&str> =
                            children.iter().take(3).map(|(n, _)| n.as_str()).collect();
                        if child_count <= 3 {
                            format!("Object {{ {} }}", preview.join(", "))
                        } else {
                            format!(
                                "Object {{ {}, … }} — {} properties",
                                preview.join(", "),
                                child_count
                            )
                        }
                    }
                    PropertyKind::Array => {
                        format!("Array — {} element properties", child_count)
                    }
                    PropertyKind::Scalar => {
                        if is_temp {
                            "Scalar — scoped to passage".to_string()
                        } else {
                            "Scalar".to_string()
                        }
                    }
                    PropertyKind::Unknown => {
                        if is_temp {
                            "Temp variable — scoped to passage".to_string()
                        } else {
                            "Story variable — persists across passages".to_string()
                        }
                    }
                };

                // Sort: Objects first (most completable), then Arrays,
                // then Scalars, then Unknowns. Within each group, sort
                // alphabetically by name.
                let sort_prefix = match inferred_kind {
                    PropertyKind::Object => "0",
                    PropertyKind::Array => "1",
                    PropertyKind::Scalar => "2",
                    PropertyKind::Unknown => "3",
                };

                // Text edit: replace partial after sigil with full name.
                // The `$` or `_` stays in the document (like `<<` for
                // macros) — only the identifier portion is replaced.
                let text_edit = compute_variable_text_edit(partial, line, character, &filter_name);

                // Commit characters: add "." for chaining into properties
                // on Object/Array variables. Scalars don't need it.
                let commit_chars = match inferred_kind {
                    PropertyKind::Object | PropertyKind::Array => {
                        vec![".".to_string(), " ".to_string(), "\n".to_string()]
                    }
                    PropertyKind::Scalar | PropertyKind::Unknown => {
                        vec![" ".to_string(), "\n".to_string()]
                    }
                };

                // Data payload for resolve: carry structural info so
                // completion_resolve can show type, children, def-site.
                let kind_str = match inferred_kind {
                    PropertyKind::Object => "object",
                    PropertyKind::Array => "array",
                    PropertyKind::Scalar => "scalar",
                    PropertyKind::Unknown => "unknown",
                };
                let child_names: Vec<&str> =
                    children.iter().take(10).map(|(n, _)| n.as_str()).collect();

                FormatCompletionItem {
                    label: display_name.clone(),
                    kind: completion_kind,
                    detail: Some(detail),
                    sort_text: Some(format!("{}_{}", sort_prefix, name)),
                    filter_text: Some(filter_name.clone()),
                    // insert_text is the fallback when text_edit is None;
                    // insert just the name (sigil already in document).
                    insert_text: Some(filter_name.clone()),
                    insert_text_format: FormatInsertTextFormat::PlainText,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({
                        "type": "variable",
                        "name": name,
                        "is_temp": is_temp,
                        "inferred_kind": kind_str,
                        "child_count": child_count,
                        "child_names": child_names,
                    })),
                    commit_characters: commit_chars,
                }
            })
            .collect()
    }

    /// Build macro completions including builtins + custom macros.
    ///
    /// Uses the multi-form completion system: each macro can declare multiple
    /// completion forms (e.g., `<<link>>` has 5 forms for different arg counts
    /// and inline/block variants). Forms come from `completion_forms.rs`.
    /// Macros without explicit forms fall back to `build_macro_snippet()`.
    ///
    /// Uses `textEdit` to properly consume and replace `<<partial` text.
    /// When `filter_prefix` is non-empty, only macros whose name starts with
    /// the prefix are included.
    pub(super) fn build_macro_completions(
        &self,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
        filter_prefix: &str,
        cursor: CursorPos,
        text: &str,
        byte_offset: usize,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, MacroKind,
        };

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Compute text after cursor (for auto-close `>>` detection) ──
        let after_cursor = &text[byte_offset..];

        // ── Determine enclosing block macros for sub-macro filtering ──
        //
        // Phase 4 migration: replaced `find_enclosing_block_macros(text, byte_offset, &body_macros)`
        // (an O(document-length) text scan from byte 0 to cursor) with a zone-map
        // query (`passage.zones.body_stack_at`), which is O(depth).
        //
        // The zone map's `body_stack_at` returns all ancestor macro bodies
        // (outermost-first). We filter to bodies whose `body_requirement` is
        // `Required` or `Optional` (i.e., not `Never`) to match the old
        // `body_macro_names()` filter.
        //
        // **Fallback**: if the zone map returns an empty stack (which can
        // happen when the user is mid-typing an incomplete `<<` that the
        // parser pairs unexpectedly), fall back to the old text-scan. This
        // preserves the old behavior for incomplete input — the text-scan
        // is more forgiving of incomplete macros because it doesn't build
        // a tree, it just tracks open/close pairs.
        let body_macros = self.body_macro_names();
        let enclosing: Vec<String> = {
            let zone_result = if let Some(doc) = workspace.get_document(uri) {
                let passage = doc
                    .passages
                    .iter()
                    .find(|p| p.contains_abs_offset(byte_offset));
                if let Some(passage) = passage {
                    let passage_offset = byte_offset.saturating_sub(passage.passage_offset);
                    passage
                        .zones
                        .body_stack_at(passage_offset)
                        .into_iter()
                        .filter(|body| {
                            matches!(
                                body.body_requirement,
                                Some(crate::types::BodyRequirement::Required)
                                    | Some(crate::types::BodyRequirement::Optional)
                            )
                        })
                        .map(|body| body.macro_name.clone())
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            if !zone_result.is_empty() {
                zone_result
            } else {
                // Fallback to text-scan for incomplete input.
                #[allow(deprecated)]
                find_enclosing_block_macros(text, byte_offset, &body_macros)
            }
        };
        // ── Builtin macros (version-filtered) ──────────────────────
        //
        // The completion list is filtered through `macros_at` — the
        // version gate over the union catalog — so only macros that exist
        // (available or deprecated) at the story's pinned `format-version`
        // are offered. Removed macros (e.g. `<<click>>` at 2.37+) and
        // not-yet-added macros (e.g. `<<type>>` before 2.32) are excluded,
        // and each item's detail/description comes from its era-effective
        // descriptor. The parent-constraints map (sub-macro scoping) is
        // filtered through the same gate: a sub-macro whose macro does
        // not exist at this version is not a relevant element to scope.
        let story_version = self.effective_story_version();
        let parent_constraints = macros::macro_parent_constraints_at(story_version);
        for (mdef, descriptor) in macros::macros_at(story_version) {
            if !filter_prefix.is_empty() && !mdef.name.starts_with(filter_prefix) {
                continue;
            }
            let is_deprecated = mdef.is_deprecated_at(story_version);

            // Phase 2: Sub-macro scoping — filter SubMacro items when the
            // cursor is not inside a valid parent container.
            if mdef.kind == MacroKind::SubMacro {
                // Look up which parents this sub-macro requires
                if let Some(valid_parents) = parent_constraints.get(mdef.name) {
                    // Check if ANY enclosing macro is a valid parent
                    let inside_valid_parent = enclosing
                        .iter()
                        .any(|enc| valid_parents.contains(enc.as_str()));
                    if !inside_valid_parent {
                        // If the user's filter prefix partially matches this
                        // sub-macro name, still show it but deprioritize it
                        // (user might be typing it intentionally).
                        if filter_prefix.is_empty() || !mdef.name.starts_with(filter_prefix) {
                            continue;
                        }
                        // Partial match — include but deprioritize (sort prefix "9_")
                        // Fall through to normal processing with sort adjustment below
                    }
                }
            }

            seen.insert(mdef.name.to_string());

            let category = mdef.category.to_string();

            // Determine sort prefix (lower = higher priority in completion list):
            // - "0" = context-smart: sub-macro inside its valid parent (top priority)
            // - "1" = normal macro (default priority)
            // - "2" = deprecated macro (still shown but after non-deprecated)
            // - "9" = sub-macro outside valid parent (lowest priority)
            let sort_prefix = if mdef.kind == MacroKind::SubMacro {
                // Check if this was a partial-prefix match outside valid parent
                if let Some(valid_parents) = parent_constraints.get(mdef.name) {
                    let inside_valid_parent = enclosing
                        .iter()
                        .any(|enc| valid_parents.contains(enc.as_str()));
                    if !inside_valid_parent {
                        "9" // Deprioritized — outside valid parent
                    } else {
                        "0" // Context-smart: inside valid parent, boost to top
                    }
                } else {
                    "1"
                }
            } else if is_deprecated {
                "2" // Deprecated — shown after normal macros
            } else {
                "1"
            };

            // Check for multi-form completions first
            if let Some(forms) = macros::macro_completion_forms(mdef.name) {
                for form in forms {
                    let snippet = macros::convert_snippet_newlines(form.snippet);
                    let text_edit =
                        compute_macro_text_edit(filter_prefix, cursor, &snippet, after_cursor);
                    let detail_text = if is_deprecated {
                        format!("[Deprecated] [{}] {}", category, form.detail)
                    } else {
                        format!("[{}] {}", category, form.detail)
                    };
                    items.push(FormatCompletionItem {
                        label: form.label.to_string(),
                        kind: FormatCompletionKind::Function,
                        detail: Some(detail_text),
                        sort_text: Some(format!(
                            "{}_{}_{:02}",
                            sort_prefix, mdef.name, form.sort_priority
                        )),
                        filter_text: Some(mdef.name.to_string()),
                        insert_text: Some(snippet),
                        insert_text_format: FormatInsertTextFormat::Snippet,
                        text_edit,
                        deprecated: is_deprecated,
                        preselect: form.sort_priority == 0
                            && (sort_prefix == "0" || sort_prefix == "1"),
                        data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
                        commit_characters: Vec::new(),
                    });
                }
            } else {
                // Single-form macro: use the existing snippet system
                let snippet = self.build_macro_snippet(mdef.name, mdef.body);
                let text_edit =
                    compute_macro_text_edit(filter_prefix, cursor, &snippet, after_cursor);
                let detail_text = if is_deprecated {
                    format!("[Deprecated] [{}] {}", category, descriptor.description)
                } else {
                    format!("[{}] {}", category, descriptor.description)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", mdef.name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(detail_text),
                    sort_text: Some(format!("{}_{}_00", sort_prefix, mdef.name)),
                    filter_text: Some(mdef.name.to_string()),
                    insert_text: Some(snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit,
                    deprecated: is_deprecated,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
                    commit_characters: Vec::new(),
                });
            }
        }

        // ── Custom macros (widgets, Macro.add) ────────────────────────
        let custom_names = self.registry.custom_macro_names();
        for name in &custom_names {
            if !filter_prefix.is_empty() && !name.starts_with(filter_prefix) {
                continue;
            }
            if seen.contains(name) {
                continue;
            }
            seen.insert(name.clone());

            let custom = self.registry.custom_macros().get(name);
            let is_widget = custom.map(|m| m.is_widget).unwrap_or(false);
            let is_container = custom.map(|m| m.is_container()).unwrap_or(false);
            let arg_count = custom.and_then(|m| m.arg_count);
            let description = custom.and_then(|m| m.description.as_deref());

            // Build detail text
            let detail_base = if is_widget {
                format!("Custom widget — {}", name)
            } else {
                format!("Custom macro — {}", name)
            };

            // Container widgets: offer block form only with _contents tabstop
            // Non-container widgets: offer block form as primary, inline form as secondary
            if is_widget && is_container {
                // Container widget: block form only with _contents at $2
                let block_snippet = macros::convert_snippet_newlines(&format!(
                    "{} $1>>\\n$2\\n<</{}>>",
                    name, name
                ));
                let block_text_edit =
                    compute_macro_text_edit(filter_prefix, cursor, &block_snippet, after_cursor);
                let block_detail = if let Some(desc) = description {
                    format!("{} (container) — {}", name, desc)
                } else {
                    format!("{} (container)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>…<</{}>>", name, name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", block_detail)),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(block_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: block_text_edit,
                    deprecated: false,
                    preselect: true,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            } else if is_widget {
                // Block form: <<name>>…<</name>>
                let block_snippet = macros::convert_snippet_newlines(&format!(
                    "{} $1>>\\n$2\\n<</{}>>",
                    name, name
                ));
                let block_text_edit =
                    compute_macro_text_edit(filter_prefix, cursor, &block_snippet, after_cursor);
                let block_detail = if let Some(desc) = description {
                    format!("{} (with body) — {}", name, desc)
                } else {
                    format!("{} (with body)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>…<</{}>>", name, name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", block_detail)),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(block_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: block_text_edit,
                    deprecated: false,
                    preselect: true,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });

                // Inline form: <<name args>> or <<name>> (if argless)
                let arg_placeholder = if let Some(n) = arg_count {
                    if n == 0 {
                        // Argless widget — no placeholder, no trailing space.
                        String::new()
                    } else {
                        (0..n)
                            .map(|i| format!("${{{}:arg{}}}", i + 1, i + 1))
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                } else {
                    // Unknown arg_count — don't insert a literal "args" placeholder.
                    // The user can type args manually. Inserting "${1:args}" would
                    // force them to delete "args" every time, which is bad UX.
                    String::new()
                };
                let inline_snippet = if arg_placeholder.is_empty() {
                    format!("{}>>", name)
                } else {
                    format!("{} {}>>", name, arg_placeholder)
                };
                let inline_text_edit =
                    compute_macro_text_edit(filter_prefix, cursor, &inline_snippet, after_cursor);
                let inline_detail = if let Some(desc) = description {
                    format!("{} (inline) — {}", name, desc)
                } else {
                    format!("{} (inline)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", inline_detail)),
                    sort_text: Some(format!("2_{}_01", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(inline_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: inline_text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            } else {
                // Non-widget custom macro (Macro.add): just inline form
                let arg_placeholder = if let Some(n) = arg_count {
                    if n == 0 {
                        String::new()
                    } else {
                        (0..n)
                            .map(|i| format!("${{{}:arg{}}}", i + 1, i + 1))
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                } else {
                    // Unknown arg_count — no literal "args" placeholder.
                    String::new()
                };
                let snippet = if arg_placeholder.is_empty() {
                    format!("{}>>", name)
                } else {
                    format!("{} {}>>", name, arg_placeholder)
                };
                let text_edit =
                    compute_macro_text_edit(filter_prefix, cursor, &snippet, after_cursor);
                let full_detail = if let Some(desc) = description {
                    format!("{} — {}", detail_base, desc)
                } else {
                    detail_base
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(full_detail),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            }
        }

        items
    }

    /// Build passage name completions with context-aware detail and text_edit.
    ///
    /// ## Context-aware detail
    ///
    /// Instead of the generic "Passage" label, each item's `detail` reflects the
    /// macro context that triggered the completion:
    ///
    /// - `<<goto "Passage">>` → "Navigation target for \<\<goto\>\>"
    /// - `<<include "Passage">>` → "Included passage for \<\<include\>\>"
    /// - `<<link "Talk" "Passage">>` → "Link target for \<\<link\>\>"
    /// - `[[Passage]]` → "Link target"
    ///
    /// The detail is derived from the `LinkSource` semantics and the macro
    /// catalog's `MacroDef.description`, following format isolation — no
    /// format-specific data leaks out of the plugin.
    ///
    /// ## text_edit
    ///
    /// Passage completions provide `text_edit` ranges that replace only the
    /// partial passage name text, not the surrounding delimiters (`[[`, `]]`,
    /// or quotes). This follows the same design as `compute_macro_text_edit`:
    ///
    /// - For `[[Partial]]`: text_edit replaces `Partial` with the full name.
    ///   The `[[` and `]]` remain in the document.
    /// - For `<<goto "Partial">>`: text_edit replaces `Partial` inside the
    ///   quotes. The quotes and `<<goto` remain.
    ///   Build template name completions for the `?` trigger.
    ///
    /// SugarCube templates are invoked with `?name` in prose. This method
    /// queries `self.registry.template_completion_names()` (which returns
    /// names WITH the `?` prefix, e.g., `?heal`) and filters by the partial
    /// name the user has typed after `?`.
    pub(super) fn build_template_completions(
        &self,
        line: u32,
        character: u32,
        partial: &str,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        let all_templates = self.registry.template_completion_names();
        let mut items = Vec::new();

        for tmpl_name in &all_templates {
            // `tmpl_name` includes the `?` prefix (e.g., "?heal").
            // Strip it for filtering against `partial` (which is without `?`).
            let name_without_prefix = tmpl_name.strip_prefix('?').unwrap_or(tmpl_name);

            // Filter: if partial is non-empty, only show templates whose name
            // starts with the partial (case-insensitive).
            if !partial.is_empty()
                && !name_without_prefix
                    .to_lowercase()
                    .starts_with(&partial.to_lowercase())
            {
                continue;
            }

            // The text edit replaces the partial (if any) with the full name.
            // The `?` prefix is NOT part of the text edit — it's already in
            // the buffer (the user typed it as the trigger character).
            let text_edit = crate::types::FormatTextEdit {
                start_line: line,
                start_character: character - partial.len() as u32,
                end_line: line,
                end_character: character,
                new_text: name_without_prefix.to_string(),
            };

            items.push(FormatCompletionItem {
                label: format!("?{}", name_without_prefix),
                kind: FormatCompletionKind::Function,
                detail: Some("Template".to_string()),
                sort_text: Some(format!("3_{}", name_without_prefix)),
                filter_text: Some(name_without_prefix.to_string()),
                insert_text: Some(name_without_prefix.to_string()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: Some(text_edit),
                deprecated: false,
                preselect: false,
                data: Some(serde_json::json!({
                    "type": "template",
                    "name": name_without_prefix,
                })),
                commit_characters: Vec::new(),
            });
        }

        items
    }

    pub(super) fn build_passage_name_completions(
        &self,
        workspace: &knot_core::Workspace,
        target: &str,
        kind: PassageCompletionKind,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        // Build a context-aware detail string based on the completion kind.
        // This uses the macro catalog and LinkSource semantics to produce
        // descriptions that tell the user *why* they're seeing passage names.
        let detail_text = match &kind {
            PassageCompletionKind::Link => "Link target".to_string(),
            PassageCompletionKind::MacroArg { macro_name, .. } => {
                // Look up the macro in the catalog for semantic context.
                // We use the same classification as LinkSource to produce
                // human-readable descriptions for each macro category.
                match macro_name.as_str() {
                    "goto" => "Navigation target for <<goto>>".to_string(),
                    "include" | "display" => format!("Included passage for <<{}>>", macro_name),
                    "link" | "button" | "click" => format!("Link target for <<{}>>", macro_name),
                    "linkappend" | "linkprepend" | "linkreplace" | "linkrepeat" => {
                        format!("Link target for <<{}>>", macro_name)
                    }
                    "actions" => "Choice passage for <<actions>>".to_string(),
                    "back" => "Return passage for <<back>>".to_string(),
                    "return" => "Return passage for <<return>>".to_string(),
                    // Fallback for unknown macros that have passage-ref args
                    // (e.g., custom macros or newly added catalog entries)
                    other => format!("Passage target for <<{}>>", other),
                }
            }
        };

        // Look up the MacroDef to enrich completion data further.
        // If found, we can extract the arg label (e.g., "passage" vs "passageName")
        // and whether the arg is required — useful for sort priority.
        // NOTE: Currently used for future enrichment (filtering by required args,
        // macro-specific sort ordering). The lookup is cheap (static catalog scan).
        let _macro_def = match &kind {
            PassageCompletionKind::MacroArg { macro_name, .. } => self.find_macro(macro_name),
            PassageCompletionKind::Link => None,
        };

        let names = workspace.all_passage_names();
        names
            .iter()
            .map(|name| {
                let (insert_text, insert_format, commit_chars) = match &kind {
                    PassageCompletionKind::Link => {
                        // For [[ links, the insert_text replaces the partial name
                        // after [[ with the full [[Name]] pattern
                        if target.is_empty() {
                            (
                                format!("[[{}]]", name),
                                FormatInsertTextFormat::Snippet,
                                vec!["]".to_string()],
                            )
                        } else {
                            // Partial name already typed after [[ — just insert the name part
                            (
                                name.clone(),
                                FormatInsertTextFormat::PlainText,
                                vec!["]".to_string()],
                            )
                        }
                    }
                    PassageCompletionKind::MacroArg { .. } => {
                        (name.clone(), FormatInsertTextFormat::PlainText, Vec::new())
                    }
                };

                // Sort priority: context-aware ordering.
                // - "Start" passage is preselected for navigation macros
                //   (most likely target for <<goto>>, <<link>>, etc.)
                // - Exact matches on the partial target get highest priority
                // - Items are otherwise sorted by index (stable order)
                let is_exact_match = !target.is_empty() && name == target;
                let is_start = name == "Start";
                let sort_prefix = if is_exact_match {
                    "0" // Exact match — highest priority
                } else if is_start {
                    "1" // Start passage — high priority for navigation
                } else {
                    "2" // Everything else
                };

                // Build the data payload. For MacroArg, include macro context
                // so completion_resolve can provide macro-specific documentation.
                let data = match &kind {
                    PassageCompletionKind::Link => {
                        serde_json::json!({
                            "type": "passage",
                            "name": name,
                        })
                    }
                    PassageCompletionKind::MacroArg {
                        macro_name,
                        has_body,
                    } => {
                        serde_json::json!({
                            "type": "passage",
                            "name": name,
                            "macro_name": macro_name,
                            "has_body": has_body,
                        })
                    }
                };

                FormatCompletionItem {
                    label: name.clone(),
                    kind: FormatCompletionKind::Module,
                    detail: Some(detail_text.clone()),
                    sort_text: Some(format!("{}_{}", sort_prefix, name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(insert_text),
                    insert_text_format: insert_format,
                    text_edit: None, // text_edit is computed by the caller with position info
                    deprecated: false,
                    preselect: is_exact_match || is_start,
                    data: Some(data),
                    commit_characters: commit_chars,
                }
            })
            .collect()
    }

    /// Build close-tag completions for unclosed block macros.
    ///
    /// Phase 2: Now uses proper open/close pair scanning instead of the old
    /// stub that returned ALL body macro names. Only macros that are actually
    /// unclosed at the cursor position are offered as close-tag completions.
    /// Also provides `text_edit` for each close-tag item, replacing from `<</`
    /// to the cursor with `name>>`.
    pub(super) fn build_close_tag_completions(
        &self,
        partial: &str,
        text: &str,
        byte_offset: usize,
        line: u32,
        character: u32,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, FormatTextEdit,
        };

        // Find actually unclosed block macros at cursor position
        let unclosed = self.find_unclosed_block_macros(text, byte_offset);

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Compute text_edit that replaces partial after `<</` with `name>>`
        // Same design principle as compute_macro_text_edit: don't include `<</`
        // in the textEdit range because VS Code uses it as filter prefix.
        // The textEdit range covers only the partial name after `<</`.
        let compute_close_text_edit = |name: &str| -> Option<FormatTextEdit> {
            let partial_len = partial.chars().count() as u32;
            // Replace just the partial name (after `<</` and before cursor)
            let start_char = character.saturating_sub(partial_len);
            Some(FormatTextEdit {
                start_line: line,
                start_character: start_char,
                end_line: line,
                end_character: character,
                // Insert the name + `>>`. The `<</` stays in the document.
                new_text: format!("{name}>>"),
            })
        };

        // First offer close tags for actually unclosed macros
        for name in unclosed.iter().rev() {
            if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                continue;
            }
            seen.insert(name.clone());
            let text_edit = compute_close_text_edit(name);
            items.push(FormatCompletionItem {
                label: format!("/{name}>>"),
                kind: FormatCompletionKind::Function,
                detail: Some(format!("Close <<{}>>", name)),
                sort_text: Some(format!("0_{}", name)),
                filter_text: Some(name.clone()),
                insert_text: Some(format!("{name}>>")),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit,
                deprecated: false,
                preselect: false,
                data: None,
                commit_characters: Vec::new(),
            });
        }

        // If no unclosed macros found, offer all block macro close tags as
        // fallback — version-filtered, so only macros that exist at the
        // story's pinned `format-version` are offered (a 2.30 story never
        // sees `<</type>>`; a 2.37 story never sees `<</click>>`).
        if items.is_empty() {
            for name in macros::body_macro_names_at(self.effective_story_version()) {
                if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                    continue;
                }
                seen.insert(name.to_string());
                let text_edit = compute_close_text_edit(name);
                items.push(FormatCompletionItem {
                    label: format!("/{name}>>"),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Close <<{}>>", name)),
                    sort_text: Some(format!("1_{}", name)),
                    filter_text: Some(name.to_string()),
                    insert_text: Some(format!("{name}>>")),
                    insert_text_format: FormatInsertTextFormat::PlainText,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: None,
                    commit_characters: Vec::new(),
                });
            }
        }

        items
    }

    /// Build variable dot-notation completions (e.g., $player. → .name, .hp).
    ///
    /// This is the same operation as `$`/`_` completions — "show children
    /// of a node in the arena tree" — just starting from a deeper node
    /// rather than a scope root. The `var_path` identifies the parent
    /// node (e.g., `"$player"` or `"$player.address"`).
    ///
    /// Includes `text_edit` for partial property replacement and `.`
    /// commit character for continued chaining on Object/Array children.
    pub(super) fn build_variable_dot_completions(
        &self,
        var_path: &str,
        line: u32,
        character: u32,
        partial: &str,
        passage_name: Option<&str>,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, PropertyKind,
        };

        // Use passage-aware variants so temp-var dot-completion
        // (`_x.` in passage A) resolves against A's namespaced
        // path_index key, never leaking B's `_x` children.
        let children = self.variable_children_with_kind_for_passage(var_path, passage_name);
        if children.is_empty() {
            return Vec::new();
        }

        let entry_kind = self
            .variable_kind_at_path_for_passage(var_path, passage_name)
            .unwrap_or(PropertyKind::Unknown);
        let mut items = Vec::new();

        match entry_kind {
            PropertyKind::Array => {
                // Array built-in methods
                let array_props = [
                    (".length", "Array property", false),
                    (".push()", "Array method", true),
                    (".pop()", "Array method", true),
                    (".shift()", "Array method", true),
                    (".unshift()", "Array method", true),
                    (".includes()", "Array method", true),
                    (".indexOf()", "Array method", true),
                    (".splice()", "Array method", true),
                ];
                for (i, (prop, detail, is_method)) in array_props.iter().enumerate() {
                    let method_name = prop.trim_start_matches('.');
                    let text_edit =
                        compute_variable_text_edit(partial, line, character, method_name);
                    items.push(FormatCompletionItem {
                        label: method_name.to_string(),
                        kind: if *is_method {
                            FormatCompletionKind::Method
                        } else {
                            FormatCompletionKind::Property
                        },
                        detail: Some(format!("{} of {}", detail, var_path)),
                        sort_text: Some(format!("0_{:06}_{}", i, prop)),
                        filter_text: Some(method_name.to_string()),
                        insert_text: Some(method_name.to_string()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": method_name,
                            "is_method": is_method,
                        })),
                        commit_characters: Vec::new(),
                    });
                }
                // Element properties
                for (i, (child_name, child_kind)) in children.iter().enumerate() {
                    let detail = match child_kind {
                        PropertyKind::Object => format!("Object property of {}", var_path),
                        PropertyKind::Array => format!("Array property of {}", var_path),
                        _ => format!("Element property of {}", var_path),
                    };
                    let insert = format!("[0].{}", child_name);
                    let text_edit = compute_variable_text_edit(partial, line, character, &insert);
                    // Offer "." commit for object/array element properties
                    let commit_chars = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => {
                            vec![".".to_string()]
                        }
                        _ => Vec::new(),
                    };
                    items.push(FormatCompletionItem {
                        label: format!("[0].{}", child_name),
                        kind: FormatCompletionKind::Property,
                        detail: Some(detail),
                        sort_text: Some(format!("1_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(insert.clone()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": child_name,
                            "is_method": false,
                        })),
                        commit_characters: commit_chars,
                    });
                }
            }
            PropertyKind::Object | PropertyKind::Unknown => {
                for (i, (child_name, child_kind)) in children.iter().enumerate() {
                    let kind = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => FormatCompletionKind::Module,
                        _ => FormatCompletionKind::Field,
                    };
                    let detail = match child_kind {
                        PropertyKind::Object => format!("Object property of {}", var_path),
                        PropertyKind::Array => format!("Array property of {}", var_path),
                        _ => format!("Property of {}", var_path),
                    };
                    let text_edit =
                        compute_variable_text_edit(partial, line, character, child_name);
                    // Offer "." commit for object/array children so the
                    // user can chain deeper (e.g., $player.address.)
                    let commit_chars = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => {
                            vec![".".to_string()]
                        }
                        _ => Vec::new(),
                    };
                    let kind_str = match child_kind {
                        PropertyKind::Object => "object",
                        PropertyKind::Array => "array",
                        PropertyKind::Scalar => "scalar",
                        PropertyKind::Unknown => "unknown",
                    };
                    items.push(FormatCompletionItem {
                        label: child_name.clone(),
                        kind,
                        detail: Some(detail),
                        sort_text: Some(format!("0_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(child_name.clone()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": child_name,
                            "inferred_kind": kind_str,
                            "is_method": false,
                        })),
                        commit_characters: commit_chars,
                    });
                }
            }
            PropertyKind::Scalar => {}
        }

        items
    }

    /// Build global object property completions (e.g., State. → .variables, .passage).
    pub(super) fn build_global_property_completions(
        &self,
        object_name: &str,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        if !self.global_object_names().contains(object_name) {
            return Vec::new();
        }

        let global_def = match self
            .builtin_globals()
            .iter()
            .find(|g| g.name == object_name)
        {
            Some(g) => g,
            None => return Vec::new(),
        };

        let properties = match global_def.properties {
            Some(props) => props,
            None => return Vec::new(),
        };

        properties
            .iter()
            .map(|prop| FormatCompletionItem {
                label: prop.name.to_string(),
                kind: if prop.is_method {
                    FormatCompletionKind::Method
                } else {
                    FormatCompletionKind::Property
                },
                detail: Some(prop.description.to_string()),
                sort_text: None,
                filter_text: Some(prop.name.to_string()),
                insert_text: Some(prop.name.to_string()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: None,
                commit_characters: Vec::new(),
            })
            .collect()
    }

    /// Build default completions when no specific context is detected.
    ///
    /// Offers workspace symbols: variables, custom macros, and JS globals.
    ///
    /// **NOT included**: Passage names. They are only valid inside `[[link]]`
    /// syntax or macro passage-ref args — both contexts are handled by
    /// earlier branches in `provide_completions` (trigger-based and
    /// semantic-token-based). Including them here caused false passage
    /// suggestions in plain prose.
    pub(super) fn build_default_completions(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Story variables ──────────────────────────────────────────
        //
        // NOTE: Passage names are deliberately NOT included in the default
        // completions. Passage names are only valid in specific contexts:
        //   - Inside `[[link]]` syntax (handled by the `[` trigger branch)
        //   - Inside macro passage-ref args (handled by the `"` trigger and
        //     semantic-token branches in `provide_completions`)
        //
        // Including passage names in the default fallback caused them to
        // appear when the user hit Ctrl+Space in plain prose — confusing
        // because bare passage names in prose are just text, not references.
        // All valid passage-name contexts are handled by earlier branches
        // in `provide_completions`, so the default fallback doesn't need them.
        let var_names = self.registry.variable_names();
        let mut sorted_vars: Vec<_> = var_names.into_iter().collect();
        sorted_vars.sort();
        for (i, name) in sorted_vars.iter().enumerate() {
            if name.starts_with('_') {
                continue; // Skip temp vars in default context
            }
            let key = format!("var:{}", name);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            let display = if name.starts_with('$') {
                name.clone()
            } else {
                format!("${name}")
            };
            items.push(FormatCompletionItem {
                label: display.clone(),
                kind: FormatCompletionKind::Variable,
                detail: Some("Story variable".to_string()),
                sort_text: Some(format!("1_{:06}", i)),
                filter_text: Some(name.trim_start_matches('$').to_string()),
                insert_text: Some(display),
                insert_text_format: FormatInsertTextFormat::Snippet,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: Some(serde_json::json!({"type": "variable", "name": name, "is_temp": false})),
                commit_characters: Vec::new(),
            });
        }

        // ── Custom macros / widgets ──────────────────────────────────
        for name in self.registry.custom_macro_names() {
            let key = format!("macro:{}", name);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            items.push(FormatCompletionItem {
                label: format!("<<{}>>", name),
                kind: FormatCompletionKind::Function,
                detail: Some("Custom macro".to_string()),
                sort_text: Some(format!("2_{}", name)),
                filter_text: Some(name.clone()),
                insert_text: Some(name.clone()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: Some(serde_json::json!({"type": "macro", "name": name})),
                commit_characters: Vec::new(),
            });
        }

        items
    }

    /// Resolve whether the cursor is inside a passage-ref macro arg,
    /// returning the full `PassageCompletionKind::MacroArg` context if so.
    ///
    /// This replaces the old `is_in_passage_arg_quote` which only returned
    /// a boolean. Now we carry the macro_name and has_body through, which
    /// enables context-aware detail text in the completion items.
    ///
    /// Detection strategy (same as before, but richer return):
    /// 1. Span-based: check MacroArgRef at cursor offset (most accurate)
    /// 2. Line-based: detect passage-in-quote pattern (AST lag fallback)
    pub(super) fn resolve_passage_arg_context(
        &self,
        before_cursor: &str,
        byte_offset: usize,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
    ) -> Option<PassageCompletionKind> {
        // First try span-based detection — gives us macro_name and has_body
        if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
            return Some(PassageCompletionKind::MacroArg {
                macro_name: arg_ref.macro_name.clone(),
                has_body: arg_ref.has_body,
            });
        }
        // Fallback to line-based detection
        if let Some(ctx) = detect_passage_in_quote(before_cursor, self) {
            let (macro_name, has_body) = match ctx {
                crate::types::CompletionContext::MacroPassageRef {
                    macro_name,
                    has_body,
                    ..
                } => (macro_name, has_body),
                _ => return None,
            };
            return Some(PassageCompletionKind::MacroArg {
                macro_name,
                has_body,
            });
        }
        None
    }

    /// Find unclosed block macros by scanning text for open/close pairs.
    ///
    /// Phase 2: Now properly scans the source text using `scan_macro_tags()`
    /// and `build_open_macro_stack_at_offset()` instead of the old stub that
    /// returned ALL body macro names. Returns only macros that are actually
    /// unclosed at the cursor position, from outermost to innermost.
    fn find_unclosed_block_macros(&self, text: &str, byte_offset: usize) -> Vec<String> {
        let body_macros = self.body_macro_names();
        build_open_macro_stack_at_offset(text, byte_offset, &body_macros)
    }
}
