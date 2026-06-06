//! SugarCube 2.x Format Plugin — Rewrite (ver_3)
//!
//! This module is being rewritten from scratch. The old implementation had
//! ~2500 lines of regex spaghetti spread across vars/, links/, validation/,
//! macro_scan/, workspace/, comments/, and passage_tree/. This rewrite replaces
//! all of that with a single recursive descent parser that handles SugarCube's
//! delimiter-based syntax natively.
//!
//! ## Architecture
//!
//! ```text
//! Source Text
//!     |
//!     v
//! lexer::split_passages()     ← Passage boundary detection (kept from old code)
//!     |
//!     v
//! classifier::classify_all()  ← Two-pass: detect + classify (tags-first per Twee 3)
//!     |
//!     v
//! classifier::sort_for_processing() ← Define-before-use ordering
//!     |
//!     v
//! [per-passage dispatch]       ← Each category gets the right parser mode
//!     |
//!     |--> Script:         oxc parse → warm registries
//!     |--> Widget:         SC parser (Widget mode) → warm widget registry
//!     |--> Normal/Special: SC parser (Normal mode)
//!     |--> Stylesheet:     skip
//!     |--> StoryData:      minimal
//!     |
//!     v
//! ParseResult { passages, tokens, diagnostics }
//! ```
//!
//! ## Classification Priority (Twee 3 spec: tags override names)
//!
//! 1. Core name-matched (StoryTitle, StoryData, Start)
//! 2. Core tag-matched ([script], [stylesheet])
//! 3. Format tag-matched ([init], [widget])
//! 4. Format name-matched (StoryInit, PassageHeader, etc.)
//! 5. Normal passages (with or without custom tags)
//!
//! ## Processing Order (define-before-use)
//!
//! 1. [script] passages → oxc → populate variable/macro registries
//! 2. [widget] passages → SugarCube parser → populate widget registry
//! 3. Named specials → SugarCube parser (registries now warm)
//! 4. Normal passages → SugarCube parser (can query all registries)
//! 5. Stylesheets/StoryData → skip or minimal processing

pub mod lexer;
pub mod special_passages;
pub mod macros;
pub mod classifier;
pub mod ast;
pub mod parser;
pub mod variable_tree;
pub mod custom_macros;
pub mod js_preprocess;
pub mod js_walk;

use knot_core::passage::{Passage, SpecialPassageDef, StoryFormat, VarKind, VarOp, Block, PassageCategory as CorePassageCategory};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use url::Url;

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, FormatPlugin, ParseResult, SemanticToken, SemanticTokenModifier, SemanticTokenType};
use crate::types::{
    GlobalDef, ImplicitPassagePattern, MacroDef, OperatorNormalization,
    VariableSigilInfo, VariableTreeNode,
};
use ast::{ParseMode, PassageAst};
use classifier::{ClassifiedPassage, is_script_passage, is_stylesheet_passage, is_widget_passage};
use variable_tree::VariableTree;
use custom_macros::CustomMacroRegistry;

/// SugarCube 2.x format plugin.
///
/// Format-owned registries are exposed through trait methods so that
/// LSP handlers never touch VariableTree/CustomMacroRegistry directly.
pub struct SugarCubePlugin {
    /// Side table tracking all `$var` / `_var` references across the workspace.
    variable_tree: RwLock<VariableTree>,
    /// Registry of user-defined macros (widgets and `Macro.add()` calls).
    custom_macros: RwLock<CustomMacroRegistry>,
}

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SugarCubePlugin {
    pub fn new() -> Self {
        Self {
            variable_tree: RwLock::new(VariableTree::new()),
            custom_macros: RwLock::new(CustomMacroRegistry::new()),
        }
    }

    /// Determine the parse mode for a classified passage.
    fn parse_mode_for(cp: &ClassifiedPassage) -> ParseMode {
        if is_script_passage(cp) {
            ParseMode::Script
        } else if is_stylesheet_passage(cp) {
            ParseMode::Stylesheet
        } else if is_widget_passage(cp) {
            ParseMode::Widget
        } else if cp.header.name == "StoryInterface" {
            ParseMode::Interface
        } else if cp.header.name == "StoryData" {
            ParseMode::Minimal
        } else {
            ParseMode::Normal
        }
    }

    /// Build a `Passage` from a classified passage and its AST.
    fn build_passage(cp: &ClassifiedPassage, passage_ast: &PassageAst, body_offset: usize) -> Passage {
        let is_special = cp.special_def.is_some();
        let mut passage = if is_special {
            Passage::new_special(
                cp.header.name.clone(),
                cp.header.header_start..cp.header.header_start + 0, // span computed in caller
                cp.special_def.clone().unwrap(),
            )
        } else {
            Passage::new(cp.header.name.clone(), cp.header.header_start..cp.header.header_start + 0)
        };

        passage.tags = cp.header.tags.clone();

        // Build body blocks from AST (shift spans by body_offset)
        passage.body = build_body_blocks(&passage_ast.nodes, body_offset);

        // Build links from AST (shift spans by body_offset)
        passage.links = passage_ast.links.iter().map(|link_info| {
            knot_core::passage::Link {
                display_text: link_info.display.clone(),
                target: link_info.target.clone(),
                span: body_offset + link_info.span.start..body_offset + link_info.span.end,
                edge_type_hint: None, // Will be classified later by navigation
            }
        }).collect();

        // Build var ops from AST (shift spans by body_offset)
        passage.vars = passage_ast.var_ops.iter().map(|var_op| {
            VarOp {
                name: var_op.name.clone(),
                kind: if var_op.is_write { VarKind::Init } else { VarKind::Read },
                span: body_offset + var_op.span.start..body_offset + var_op.span.end,
                is_temporary: var_op.is_temporary,
            }
        }).collect();

        passage
    }

    /// Populate registries from a parsed passage AST.
    ///
    /// This walks the AST's var_ops and links to feed the VariableTree
    /// and CustomMacroRegistry side tables. Called during the ordered
    /// parse pipeline so that registries are warm for later passages.
    fn populate_registries_from_ast(
        &self,
        passage_ast: &PassageAst,
        cp: &ClassifiedPassage,
        file_uri: &str,
        _body_offset: usize,
    ) {
        let mut var_tree = self.variable_tree.write().unwrap();
        let mut macro_reg = self.custom_macros.write().unwrap();

        // Record variable operations from the AST
        for var_op in &passage_ast.var_ops {
            var_tree.record_var(
                &var_op.name,
                var_op.is_temporary,
                var_op.is_write,
                &cp.header.name,
                file_uri,
                var_op.span.clone(),
                &var_op.property_path,
            );
        }

        // Extract widget definitions from AST nodes
        for node in &passage_ast.nodes {
            if let ast::AstNode::Macro { name, args, open_span, .. } = node {
                // <<widget name>> definitions
                if name.eq_ignore_ascii_case("widget") {
                    let widget_name = args.trim().to_string();
                    if !widget_name.is_empty() {
                        macro_reg.register_widget(
                            &widget_name,
                            &cp.header.name,
                            file_uri,
                            open_span.start,
                            None,
                        );
                    }
                }
            }
        }

        // Mark variables as seeded if this is a special passage
        if cp.special_def.as_ref().map_or(false, |d| {
            matches!(d.behavior, knot_core::passage::SpecialPassageBehavior::Startup)
        }) {
            for var_op in &passage_ast.var_ops {
                if var_op.is_write {
                    var_tree.mark_seeded(&var_op.name);
                }
            }
        }
    }

    /// Walk JS in a script passage using oxc for deep registry population.
    ///
    /// Script passages contain full JS programs. We preprocess the `$var`
    /// references, parse with oxc, and walk the AST to find:
    /// - `State.variables.x = value` → variable writes
    /// - `Macro.add("name", {...})` → custom macro definitions
    /// - Function declarations → function registry
    fn walk_script_js(
        &self,
        body_text: &str,
        cp: &ClassifiedPassage,
        file_uri: &str,
    ) {
        use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};

        // Preprocess $var references for oxc
        let preprocessed = js_preprocess::preprocess_for_oxc(body_text);

        // Parse with oxc as a JS module
        match parse_js(&preprocessed.source, JsParseMode::Module) {
            JsParseOutcome::Success(output) => {
                let mut var_tree = self.variable_tree.write().unwrap();
                let mut macro_reg = self.custom_macros.write().unwrap();

                output.with_program(|program| {
                    js_walk::walk_script_passage(
                        program,
                        &preprocessed,
                        file_uri,
                        &cp.header.name,
                        &mut var_tree,
                        &mut macro_reg,
                    );
                });
            }
            JsParseOutcome::Error(_diagnostics) => {
                // JS syntax errors are reported as diagnostics by the
                // caller — we just skip registry population for broken JS
            }
        }
    }

    /// Get the variable tree for read-only access.
    pub fn variable_tree(&self) -> std::sync::RwLockReadGuard<'_, VariableTree> {
        self.variable_tree.read().unwrap()
    }

    /// Get the custom macro registry for read-only access.
    pub fn custom_macro_registry(&self) -> std::sync::RwLockReadGuard<'_, CustomMacroRegistry> {
        self.custom_macros.read().unwrap()
    }
}

/// Build `Block` list from AST nodes (backward compatibility).
fn build_body_blocks(nodes: &[ast::AstNode], body_offset: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    for node in nodes {
        match node {
            ast::AstNode::Text { content, span, .. } => {
                if !content.is_empty() {
                    blocks.push(Block::Text {
                        content: content.clone(),
                        span: body_offset + span.start..body_offset + span.end,
                    });
                }
            }
            ast::AstNode::Macro { name, args, full_span, .. } => {
                blocks.push(Block::Macro {
                    name: name.clone(),
                    args: args.clone(),
                    span: body_offset + full_span.start..body_offset + full_span.end,
                });
            }
            ast::AstNode::Expression { content, span, .. } => {
                blocks.push(Block::Expression {
                    content: content.clone(),
                    span: body_offset + span.start..body_offset + span.end,
                });
            }
            ast::AstNode::Link { .. } => {
                // Links in body are represented as text blocks for backward compat
                // The actual Link data is in passage.links
            }
            ast::AstNode::Comment { .. } => {
                // Comments don't produce body blocks
            }
            ast::AstNode::Error { message, span } => {
                blocks.push(Block::Incomplete {
                    content: message.clone(),
                    span: body_offset + span.start..body_offset + span.end,
                });
            }
        }
    }
    blocks
}

/// Build semantic tokens from AST nodes.
fn build_semantic_tokens(nodes: &[ast::AstNode], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for node in nodes {
        match node {
            ast::AstNode::Macro { name: _, name_span, var_refs, children, .. } => {
                // Macro name token
                tokens.push(SemanticToken {
                    start: body_offset + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });
                // Variable references in args
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if vr.is_write { Some(SemanticTokenModifier::Definition) } else { None },
                    });
                }
                // Recurse into block macro children
                if let Some(ch) = children {
                    build_semantic_tokens(ch, tokens, body_offset);
                }
            }
            ast::AstNode::Link { target, span, .. } => {
                // Link target token
                tokens.push(SemanticToken {
                    start: body_offset + span.start + 2, // past [[
                    length: target.len(),
                    token_type: SemanticTokenType::Link,
                    modifier: None,
                });
            }
            ast::AstNode::Expression { kind: _, span: _, var_refs, .. } => {
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
            }
            ast::AstNode::Comment { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::Comment,
                    modifier: None,
                });
            }
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
            }
            ast::AstNode::Error { .. } => {}
        }
    }
}

/// Build diagnostics from AST error nodes.
fn build_diagnostics(nodes: &[ast::AstNode], diagnostics: &mut Vec<FormatDiagnostic>, body_offset: usize) {
    for node in nodes {
        if let ast::AstNode::Error { message, span } = node {
            diagnostics.push(FormatDiagnostic {
                range: body_offset + span.start..body_offset + span.end,
                message: message.clone(),
                severity: FormatDiagnosticSeverity::Error,
                code: "sc-parse".to_string(),
            });
        }
        if let ast::AstNode::Macro { children, name, name_span, close_span, .. } = node {
            if children.is_some() && close_span.is_none() {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + name_span.start..body_offset + name_span.end,
                    message: format!("Unclosed block macro: <<{}>>", name),
                    severity: FormatDiagnosticSeverity::Error,
                    code: "sc-unclosed".to_string(),
                });
            }
            if let Some(ch) = children {
                build_diagnostics(ch, diagnostics, body_offset);
            }
        }
    }
}

/// Build header tokens for a passage.
fn build_header_tokens(header: &crate::header::TweeHeader, is_special: bool) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // :: prefix token
    let header_type = if is_special {
        SemanticTokenType::SpecialPassageHeader
    } else {
        SemanticTokenType::PassageHeader
    };
    tokens.push(SemanticToken {
        start: header.header_start,
        length: 2, // ::
        token_type: header_type,
        modifier: None,
    });

    // Passage name token
    let name_type = if is_special {
        SemanticTokenType::SpecialPassage
    } else {
        SemanticTokenType::PassageName
    };
    let name_len = header.name.len();
    tokens.push(SemanticToken {
        start: header.name_start,
        length: name_len,
        token_type: name_type,
        modifier: None,
    });

    // Tag tokens — only the tag names, with appropriate modifiers
    for tag in &header.tags {
        if let Some(tag_pos) = header.tags_raw.find(tag.as_str()) {
            // Classify the tag to determine its modifier
            let modifier = self_classify_tag(tag);
            tokens.push(SemanticToken {
                start: header.name_start + tag_pos,
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier,
            });
        }
    }

    tokens
}

/// Classify a tag and return the appropriate semantic token modifier.
fn self_classify_tag(tag: &str) -> Option<SemanticTokenModifier> {
    // Core tags: [script], [stylesheet], [style]
    for def in knot_core::passage::twine_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Legacy core tags
    for def in knot_core::passage::legacy_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Format-specific tags: [init], [widget]
    for def in special_passages::tag_matched_special_passages() {
        if tag.eq_ignore_ascii_case(&def.name) {
            return Some(SemanticTokenModifier::StoryFormat);
        }
    }
    None
}

impl FormatPlugin for SugarCubePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::SugarCube
    }

    fn parse(&self, uri: &Url, text: &str) -> ParseResult {
        // 1. Split into raw passages
        let raw_passages = lexer::split_passages(text);

        // 2. Classify each passage
        let mut classified = classifier::classify_all(&raw_passages, &uri.to_string());

        // 3. Sort by processing priority (scripts first, normal last)
        classifier::sort_for_processing(&mut classified);

        // 4. Clear registries for this file before re-populating
        {
            let mut var_tree = self.variable_tree.write().unwrap();
            var_tree.remove_file(&uri.to_string());
            let mut macro_reg = self.custom_macros.write().unwrap();
            macro_reg.remove_file(&uri.to_string());
        }

        // 5. Process each passage in order
        let mut passages = Vec::new();
        let mut all_tokens = Vec::new();
        let mut all_diagnostics = Vec::new();

        for cp in &classified {
            let mode = Self::parse_mode_for(cp);

            // Compute where the body starts in the document (after header line + newline)
            let header_line_end = text[cp.header.header_start..]
                .find('\n')
                .map_or(text.len(), |pos| cp.header.header_start + pos + 1);
            let body_offset = header_line_end;

            // Parse the body (parser returns offsets relative to body text start)
            let passage_ast = parser::parse_passage_body(&cp.body_text, body_offset, mode);

            // Populate registries from the AST
            self.populate_registries_from_ast(
                &passage_ast,
                &cp,
                &uri.to_string(),
                body_offset,
            );

            // For script passages, also do oxc walk for State.variables / Macro.add
            if is_script_passage(cp) {
                self.walk_script_js(&cp.body_text, &cp, &uri.to_string());
            }

            // Build the Passage struct (shift all AST spans by body_offset)
            let mut passage = Self::build_passage(cp, &passage_ast, body_offset);
            passage.span = cp.header.header_start..header_line_end + cp.body_text.len();

            // Build semantic tokens for the header
            let is_special = cp.special_def.is_some();
            let header_tokens = build_header_tokens(&cp.header, is_special);
            all_tokens.extend(header_tokens);

            // Build semantic tokens from the body AST (shift spans by body_offset)
            build_semantic_tokens(&passage_ast.nodes, &mut all_tokens, body_offset);

            // Build diagnostics from the body AST (shift spans by body_offset)
            build_diagnostics(&passage_ast.nodes, &mut all_diagnostics, body_offset);

            passages.push(passage);
        }

        ParseResult {
            passages,
            tokens: all_tokens,
            diagnostics: all_diagnostics,
            is_complete: true,
        }
    }

    fn parse_passage(&self, passage_name: &str, passage_tags: &[String], passage_text: &str) -> Option<Passage> {
        // Determine the parse mode from the tags
        let mode = if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("script")) {
            ParseMode::Script
        } else if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("stylesheet") || t.eq_ignore_ascii_case("style")) {
            ParseMode::Stylesheet
        } else if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("widget")) {
            ParseMode::Widget
        } else if passage_name == "StoryInterface" {
            ParseMode::Interface
        } else if passage_name == "StoryData" {
            ParseMode::Minimal
        } else {
            ParseMode::Normal
        };

        let passage_ast = parser::parse_passage_body(passage_text, 0, mode);

        // Classify the passage
        let (_, category) = self.classify_passage_category(passage_name, passage_tags);
        let is_special = category != CorePassageCategory::Regular;

        let mut passage = if is_special {
            let def = self.classify_passage(passage_name, passage_tags);
            Passage::new_special(
                passage_name.to_string(),
                0..passage_text.len(),
                def?,
            )
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.tags = passage_tags.to_vec();
        passage.body = build_body_blocks(&passage_ast.nodes, 0);
        passage.links = passage_ast.links.iter().map(|li| {
            knot_core::passage::Link {
                display_text: li.display.clone(),
                target: li.target.clone(),
                span: li.span.start..li.span.end,
                edge_type_hint: None,
            }
        }).collect();
        passage.vars = passage_ast.var_ops.iter().map(|vo| {
            VarOp {
                name: vo.name.clone(),
                kind: if vo.is_write { VarKind::Init } else { VarKind::Read },
                span: vo.span.start..vo.span.end,
                is_temporary: vo.is_temporary,
            }
        }).collect();

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::name_matched_special_passages()
    }

    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::tag_matched_special_passages()
    }

    fn display_name(&self) -> &str {
        "SugarCube 2"
    }

    // ── Macro catalog ──────────────────────────────────────────────────

    fn builtin_macros(&self) -> &'static [MacroDef] {
        macros::builtin_macros()
    }

    fn block_macro_names(&self) -> HashSet<&'static str> {
        macros::block_macro_names()
    }

    fn folding_modifier_names(&self) -> HashSet<&'static str> {
        macros::folding_modifier_names()
    }

    fn passage_arg_macro_names(&self) -> HashSet<&'static str> {
        macros::passage_arg_macro_names()
    }

    fn label_then_passage_macros(&self) -> HashSet<&'static str> {
        macros::label_then_passage_macros()
    }

    fn variable_assignment_macros(&self) -> HashSet<&'static str> {
        macros::variable_assignment_macros()
    }

    fn macro_definition_macros(&self) -> HashSet<&'static str> {
        macros::macro_definition_macros()
    }

    fn inline_script_macros(&self) -> HashSet<&'static str> {
        macros::inline_script_macros()
    }

    fn dynamic_navigation_macros(&self) -> HashSet<&'static str> {
        macros::dynamic_navigation_macros()
    }

    fn find_macro(&self, name: &str) -> Option<&'static MacroDef> {
        macros::find_macro(name)
    }

    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        macros::macro_parent_constraints()
    }

    fn get_passage_arg_index(&self, macro_name: &str, arg_count: usize) -> i32 {
        macros::get_passage_arg_index(macro_name, arg_count)
    }

    // ── Variable tracking ──────────────────────────────────────────────

    fn variable_sigils(&self) -> Vec<VariableSigilInfo> {
        macros::variable_sigils()
    }

    fn describe_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::describe_variable_sigil(sigil)
    }

    fn resolve_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::resolve_variable_sigil(sigil)
    }

    fn assignment_operators(&self) -> Vec<&'static str> {
        macros::assignment_operators()
    }

    fn variable_assignment_snippet(&self, var_name: &str, value: &str) -> Option<String> {
        Some(format!("<<set {} to {}>>", var_name, value))
    }

    fn comparison_operators(&self) -> Vec<&'static str> {
        macros::comparison_operators()
    }

    // ── Syntax detection ───────────────────────────────────────────────

    fn has_block_macros_with_close_tags(&self) -> bool {
        true
    }

    fn format_macro_label(&self, name: &str) -> String {
        format!("<<{}>>", name)
    }

    fn format_macro_signature_label(&self, name: &str, params: &str) -> String {
        if params.is_empty() {
            format!("<<{}>>", name)
        } else {
            format!("<<{} {}>>", name, params)
        }
    }

    fn format_close_macro_label(&self, name: &str) -> String {
        format!("<</{}>>", name)
    }

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        macros::build_macro_snippet(name, has_body)
    }

    fn detect_close_tag_context(&self, before_cursor: &str) -> Option<String> {
        if let Some(pos) = before_cursor.rfind("<</") {
            let partial = &before_cursor[pos + 3..];
            if partial.is_empty() || partial.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Some(partial.to_string());
            }
        }
        if before_cursor.ends_with("<<") {
            return Some(String::new());
        }
        None
    }

    // ── Special passage names ──────────────────────────────────────────

    fn special_passage_names(&self) -> HashSet<&'static str> {
        macros::special_passage_names()
    }

    fn system_passage_names(&self) -> HashSet<&'static str> {
        macros::system_passage_names()
    }

    // ── Implicit passage patterns ──────────────────────────────────────

    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        macros::implicit_passage_patterns()
    }

    // ── Hover / documentation ──────────────────────────────────────────

    fn global_hover_text(&self, name: &str) -> Option<&'static str> {
        macros::global_hover_text(name)
    }

    fn builtin_globals(&self) -> &'static [GlobalDef] {
        macros::builtin_globals()
    }

    fn global_object_names(&self) -> HashSet<&'static str> {
        macros::builtin_globals().iter().map(|g| g.name).collect()
    }

    // ── Operator normalization ─────────────────────────────────────────

    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        macros::operator_normalization()
    }

    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        macros::operator_precedence()
    }

    fn supports_full_variable_tracking(&self) -> bool {
        true
    }

    fn macro_snippet(&self, name: &str) -> Option<&'static str> {
        macros::macro_snippet(name)
    }

    // ── Registry accessors (Phase C) ───────────────────────────────────
    //
    // These methods expose the format-owned registries through the
    // FormatPlugin trait so that LSP handlers can query them without
    // importing format-specific types. The handlers call these methods
    // through `FormatRegistry::get()` — never directly.

    /// Build the variable tree for the workspace.
    ///
    /// Returns the current tree-structured variable inventory from the
    /// VariableTree side table. This is used by the variable tracker
    /// UI panel and by completion/hover for workspace-wide variable info.
    fn build_variable_tree(
        &self,
        _workspace: &knot_core::Workspace,
        _source_text: &dyn crate::plugin::SourceTextProvider,
    ) -> Vec<VariableTreeNode> {
        let tree = self.variable_tree.read().unwrap();
        tree.build_tree()
    }

    /// Get all workspace variable names for completion.
    fn workspace_variable_names(&self) -> HashSet<String> {
        let tree = self.variable_tree.read().unwrap();
        tree.completion_names()
    }

    /// Get known property paths for a variable (for dot-notation completion).
    fn variable_properties(&self, var_name: &str) -> HashSet<String> {
        let tree = self.variable_tree.read().unwrap();
        tree.get_variable(var_name)
            .map(|e| e.known_properties.clone())
            .unwrap_or_default()
    }

    /// Get all custom macro names for completion.
    fn custom_macro_names(&self) -> Vec<String> {
        let registry = self.custom_macros.read().unwrap();
        registry.names().cloned().collect()
    }

    /// Look up a custom macro definition for hover/go-to-def.
    fn find_custom_macro(&self, name: &str) -> Option<(String, String, usize)> {
        let registry = self.custom_macros.read().unwrap();
        registry.get(name).map(|m| {
            (m.defined_in.clone(), m.file_uri.clone(), m.defined_at_offset)
        })
    }

    /// Check if a macro name is a known custom macro.
    fn is_custom_macro(&self, name: &str) -> bool {
        let registry = self.custom_macros.read().unwrap();
        registry.contains(name)
    }
}
