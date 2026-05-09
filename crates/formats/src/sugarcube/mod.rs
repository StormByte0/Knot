//! SugarCube Format Plugin
//!
//! SugarCube 2.x is the most popular Twine story format, providing a rich macro
//! system and variable tracking via `$variable` syntax.
//!
//! This module implements a fault-tolerant, two-pass parser:
//!
//! 1. **Pass 1 — Passage boundaries**: A [`logos`]-based lexer splits the source
//!    into passage header regions and their body text.
//! 2. **Pass 2 — Body analysis**: Regex-based extractors detect links, variable
//!    operations, and macro invocations within each passage body.
//!
//! The parser never hard-fails on invalid input. Malformed constructs are captured
//! as [`Block::Incomplete`] and reported as diagnostics rather than causing panics.

pub mod macros;
pub mod lexer;
pub mod links;
pub mod vars;
pub mod tokens;
pub mod validation;
pub mod blocks;
pub mod special_passages;
pub mod comments;

use knot_core::passage::{Passage, SpecialPassageDef, StoryFormat, VarOp};
use std::collections::{HashMap, HashSet};
use url::Url;

use crate::plugin::{FormatDiagnosticSeverity, FormatPlugin, ParseResult};
use crate::types::{
    GlobalDef, ImplicitPassagePattern, MacroDef, OperatorNormalization, ResolvedNavLink,
    VariableSigilInfo,
};

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// SugarCube 2.x format plugin.
///
/// Regexes are compiled once using `once_cell::sync::Lazy` in the submodule
/// statics rather than per-instance, since they are immutable and identical
/// across all instances.
pub struct SugarCubePlugin;

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SugarCubePlugin {
    /// Create a new SugarCube plugin instance.
    ///
    /// Regexes are pre-compiled as `Lazy` statics, so this is essentially free.
    pub fn new() -> Self {
        Self
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::SugarCube
    }

    fn parse(&self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        let raw_passages = lexer::split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_start + header.header_len;

            // Determine if this is a special passage.
            let special_defs = special_passages::special_passage_defs();
            let special_def = special_defs.iter().find(|d| d.name == header.name).cloned();

            let mut passage = if let Some(def) = special_def {
                Passage::new_special(header.name.clone(), header.header_start..body_offset + body.len(), def)
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();

            // ── Context-aware parsing ──────────────────────────────────────
            // Detect script and stylesheet passages. These contain non-Twine
            // content (JavaScript or CSS) and should NOT be parsed with
            // SugarCube regexes for links, variables, or macro structure.
            //
            // Script passages: tagged [script] or named "Story JavaScript"
            // Stylesheet passages: tagged [stylesheet] or named "Story Stylesheet"
            let is_script = passage.is_script_passage();
            let is_stylesheet = passage.is_stylesheet_passage();

            if is_script {
                // Script passages: only extract implicit passage refs and
                // JS variable aliasing (Engine.play, State.variables, etc.)
                passage.links = links::extract_implicit_passage_refs(body, body_offset);
                passage.vars = vars::extract_vars(body, body_offset);
                passage.body = vec![knot_core::passage::Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                }];

                // Semantic tokens: mark entire body as a script block
                tokens.extend(tokens::header_tokens(header));

                // Validation: skip SugarCube-specific bracket checks
                // (no [[/]] or <</>> validation on JS content)
            } else if is_stylesheet {
                // Stylesheet passages: no link extraction, no variable
                // extraction — just store as a raw text block
                passage.body = vec![knot_core::passage::Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                }];

                // Semantic tokens: mark entire body as a stylesheet block
                tokens.extend(tokens::header_tokens(header));

                // No validation on CSS content
            } else {
                // Normal Twine passage: full SugarCube parsing

                // Find block comment spans so we can filter out matches
                // that fall within /* ... */ comments
                let comment_spans = comments::find_comment_spans(body);

                // Extract body elements, filtering out comment-embedded matches
                let mut raw_links = links::extract_links(body, body_offset);
                raw_links.extend(links::extract_implicit_passage_refs(body, body_offset));
                raw_links.extend(links::extract_macro_passage_refs(body, body_offset));

                // Filter links that fall inside comments
                passage.links = raw_links.into_iter().filter(|link| {
                    !comments::is_in_comment(&comment_spans, &link.span)
                }).collect();

                // Deduplicate links by (display_text, target) — the same
                // passage reference should not appear multiple times
                {
                    let mut seen = HashSet::new();
                    passage.links.retain(|link| {
                        let key = (link.display_text.clone(), link.target.clone());
                        seen.insert(key)
                    });
                }

                passage.vars = vars::extract_vars(body, body_offset);
                // Filter vars inside comments
                passage.vars.retain(|var| {
                    !comments::is_in_comment(&comment_spans, &var.span)
                });

                let macros = blocks::extract_macros(body, body_offset);
                passage.body = blocks::build_body_blocks(body, body_offset, &macros);

                // Semantic tokens for header.
                tokens.extend(tokens::header_tokens(header));

                // Semantic tokens for body (filter comment-embedded tokens)
                let mut body_tokens = tokens::body_tokens(body, body_offset);
                body_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(body_tokens);

                // Validation diagnostics (filter comment-embedded ranges)
                let body_diags = validation::validate(body, body_offset);
                let filtered_diags: Vec<_> = body_diags.into_iter().filter(|d| {
                    !comments::is_in_comment(&comment_spans, &d.range)
                }).collect();
                for d in &filtered_diags {
                    if matches!(d.severity, FormatDiagnosticSeverity::Error) {
                        has_errors = true;
                    }
                }
                diagnostics.extend(filtered_diags);
            }

            passages.push(passage);
        }

        ParseResult {
            passages,
            tokens,
            diagnostics,
            is_complete: !has_errors,
        }
    }

    fn parse_passage(&self, passage_name: &str, passage_text: &str) -> Option<Passage> {
        // For incremental re-parse: we receive just the body text.
        let special_defs = special_passages::special_passage_defs();
        let special_def = special_defs.iter().find(|d| d.name == passage_name).cloned();

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        // Context-aware: skip SugarCube regex on script/stylesheet passages
        let is_script = passage.is_script_passage();
        let is_stylesheet = passage.is_stylesheet_passage();

        if is_script {
            passage.links = links::extract_implicit_passage_refs(passage_text, 0);
            passage.vars = vars::extract_vars(passage_text, 0);
            passage.body = vec![knot_core::passage::Block::Text {
                content: passage_text.to_string(),
                span: 0..passage_text.len(),
            }];
        } else if is_stylesheet {
            passage.body = vec![knot_core::passage::Block::Text {
                content: passage_text.to_string(),
                span: 0..passage_text.len(),
            }];
        } else {
            let comment_spans = comments::find_comment_spans(passage_text);

            passage.links = links::extract_links(passage_text, 0);
            passage.links.extend(links::extract_implicit_passage_refs(passage_text, 0));
            passage.links.extend(links::extract_macro_passage_refs(passage_text, 0));

            // Filter links inside comments
            passage.links.retain(|link| {
                !comments::is_in_comment(&comment_spans, &link.span)
            });

            // Deduplicate
            {
                let mut seen = HashSet::new();
                passage.links.retain(|link| {
                    let key = (link.display_text.clone(), link.target.clone());
                    seen.insert(key)
                });
            }

            passage.vars = vars::extract_vars(passage_text, 0);
            passage.vars.retain(|var| {
                !comments::is_in_comment(&comment_spans, &var.span)
            });

            let macros = blocks::extract_macros(passage_text, 0);
            passage.body = blocks::build_body_blocks(passage_text, 0, &macros);
        }

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::special_passage_defs()
    }

    fn display_name(&self) -> &str {
        "SugarCube 2"
    }

    // -------------------------------------------------------------------
    // Macro catalog (behavioral overrides)
    // -------------------------------------------------------------------

    fn builtin_macros(&self) -> &'static [MacroDef] {
        macros::builtin_macros()
    }

    fn block_macro_names(&self) -> HashSet<&'static str> {
        macros::block_macro_names()
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

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        macros::build_macro_snippet(name, has_body)
    }

    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        macros::macro_parent_constraints()
    }

    fn get_passage_arg_index(&self, macro_name: &str, arg_count: usize) -> i32 {
        macros::get_passage_arg_index(macro_name, arg_count)
    }

    // -------------------------------------------------------------------
    // Special passages (extended)
    // -------------------------------------------------------------------

    fn special_passage_names(&self) -> HashSet<&'static str> {
        macros::special_passage_names()
    }

    fn system_passage_names(&self) -> HashSet<&'static str> {
        macros::system_passage_names()
    }

    // -------------------------------------------------------------------
    // Variable tracking
    // -------------------------------------------------------------------

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

    fn comparison_operators(&self) -> Vec<&'static str> {
        macros::comparison_operators()
    }

    // -------------------------------------------------------------------
    // Implicit passage references
    // -------------------------------------------------------------------

    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        macros::implicit_passage_patterns()
    }

    // -------------------------------------------------------------------
    // Dynamic navigation resolution
    // -------------------------------------------------------------------

    fn build_var_string_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        // SugarCube-specific: scan <<set $var to "literal">> patterns
        let re_set_string = regex::Regex::new(
            r#"<<set\s+([\$][A-Za-z_][A-Za-z0-9_]*)\s+to\s+"([^"]*)""#
        ).unwrap();

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for doc in workspace.documents() {
            for passage in &doc.passages {
                for block in &passage.body {
                    let content = match block {
                        knot_core::passage::Block::Text { content, .. } => content.as_str(),
                        knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                        _ => continue,
                    };
                    for caps in re_set_string.captures_iter(content) {
                        if let (Some(var_match), Some(val_match)) = (caps.get(1), caps.get(2)) {
                            let var_name = var_match.as_str().to_string();
                            let string_val = val_match.as_str().to_string();
                            map.entry(var_name).or_default().push(string_val);
                        }
                    }
                }
            }
        }
        for values in map.values_mut() {
            values.sort();
            values.dedup();
        }
        map
    }

    fn resolve_dynamic_navigation_links(
        &self,
        passage: &Passage,
        var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        // SugarCube-specific: resolve <<goto $var>>, <<include $var>>, <<link "label" $var>>, <<button "label" $var>>
        let re_nav_var = regex::Regex::new(
            r#"<<(?:goto|include|link|button)\s+(?:"[^"]*"\s+)?([\$][A-Za-z_][A-Za-z0-9_]*)"#
        ).unwrap();

        let mut links = Vec::new();
        for block in &passage.body {
            let content = match block {
                knot_core::passage::Block::Text { content, .. } => content.as_str(),
                knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                _ => continue,
            };
            for caps in re_nav_var.captures_iter(content) {
                if let Some(var_match) = caps.get(1) {
                    let var_name = var_match.as_str().to_string();
                    if let Some(known_values) = var_string_map.get(&var_name) {
                        for value in known_values {
                            links.push(ResolvedNavLink {
                                display_text: Some(format!("{} (via {})", value, var_name)),
                                target: value.clone(),
                            });
                        }
                    }
                }
            }
        }
        links
    }

    // -------------------------------------------------------------------
    // Hover / documentation
    // -------------------------------------------------------------------

    fn global_hover_text(&self, name: &str) -> Option<&'static str> {
        macros::global_hover_text(name)
    }

    fn builtin_globals(&self) -> &'static [GlobalDef] {
        macros::builtin_globals()
    }

    fn global_object_names(&self) -> HashSet<&'static str> {
        macros::builtin_globals().iter().map(|g| g.name).collect()
    }

    // -------------------------------------------------------------------
    // Operator normalization
    // -------------------------------------------------------------------

    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        macros::operator_normalization()
    }

    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        macros::operator_precedence()
    }

    // -------------------------------------------------------------------
    // Script/stylesheet tags
    // -------------------------------------------------------------------

    fn script_tags(&self) -> Vec<&'static str> {
        macros::script_tags()
    }

    fn stylesheet_tags(&self) -> Vec<&'static str> {
        macros::stylesheet_tags()
    }

    // -------------------------------------------------------------------
    // Macro snippet mapping
    // -------------------------------------------------------------------

    fn macro_snippet(&self, name: &str) -> Option<&'static str> {
        macros::macro_snippet(name)
    }

    // -------------------------------------------------------------------
    // Dot-notation object property map
    // -------------------------------------------------------------------

    fn build_object_property_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, HashSet<String>> {
        // Collect all variable operations across the workspace
        let vars_by_passage: Vec<Vec<&VarOp>> = workspace
            .documents()
            .flat_map(|doc| doc.passages.iter().map(|p| p.vars.iter().collect()))
            .collect();

        vars::extract_object_property_map(&vars_by_passage)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::passage::VarKind;

    #[test]
    fn parse_simple_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nYou are in a room. [[Go north->Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go north".into())
        );
        assert!(result.is_complete);
    }

    #[test]
    fn parse_multiple_passages() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
    }

    #[test]
    fn parse_passage_with_tags() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    #[test]
    fn parse_variable_operations() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>>You have $gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_pipe_link() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go to forest".into())
        );
    }

    #[test]
    fn detect_special_passages() {
        let plugin = SugarCubePlugin::new();
        assert!(plugin.is_special_passage("StoryInit"));
        assert!(plugin.is_special_passage("StoryCaption"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn unclosed_macro_diagnostic() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $x to 5\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = SugarCubePlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");

        assert!(result.passages.is_empty());
        assert!(result.is_complete);
    }

    #[test]
    fn incremental_reparse() {
        let plugin = SugarCubePlugin::new();
        let passage = plugin.parse_passage("Start", "You have $gold coins.\n");

        assert!(passage.is_some());
        let p = passage.unwrap();
        assert_eq!(p.name, "Start");
        assert!(p.vars.iter().any(|v| v.name == "$gold"));
    }

    #[test]
    fn parse_temporary_variable() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set _temp to 5>>You see _temp items.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;

        // Should detect _temp as a temporary init
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Init && v.is_temporary),
            "Should detect _temp as a temporary init"
        );

        // Should detect _temp as a temporary read
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Read && v.is_temporary),
            "Should detect _temp as a temporary read"
        );
    }

    #[test]
    fn persistent_and_temp_vars_separate() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>><<set _temp to 5>>You have $gold and _temp.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;

        // $gold should be persistent
        let gold_inits: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "$gold" && v.kind == VarKind::Init)
            .collect();
        assert_eq!(gold_inits.len(), 1);
        assert!(!gold_inits[0].is_temporary);

        // _temp should be temporary
        let temp_inits: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "_temp" && v.kind == VarKind::Init)
            .collect();
        assert_eq!(temp_inits.len(), 1);
        assert!(temp_inits[0].is_temporary);
    }

    #[test]
    fn structural_validation_else_without_if() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<else>>Some text\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<else>> outside <<if>>"
        );
    }

    #[test]
    fn structural_validation_break_without_for() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<break>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<break>> outside <<for>>"
        );
    }

    #[test]
    fn structural_validation_else_inside_if_ok() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<if $x>><<else>>OK<</if>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "<<else>> inside <<if>> should not trigger structural validation"
        );
    }

    #[test]
    fn deprecated_macro_warning() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<click \"label\" \"target\">>Click<</click>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-deprecated-macro"),
            "Should detect deprecated <<click>> macro"
        );
    }

    #[test]
    fn unknown_macro_hint() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<foobar>>test<</foobar>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-unknown-macro"),
            "Should detect unknown <<foobar>> macro"
        );
    }

    #[test]
    fn implicit_passage_ref_data_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<a data-passage=\"Forest\">Go</a>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect data-passage implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_engine_play() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>Engine.play(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Engine.play() implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_story_get() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>var p = Story.get(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Story.get() implicit reference"
        );
    }

    #[test]
    fn macro_passage_ref_goto() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<goto \"Forest\">>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect <<goto>> macro passage reference"
        );
    }

    #[test]
    fn macro_passage_ref_link() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<link \"Click\" \"Forest\">>Go<</link>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect <<link>> macro passage reference"
        );
    }

    #[test]
    fn macro_passage_ref_include() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<include \"Sidebar\">>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Sidebar"),
            "Should detect <<include>> macro passage reference"
        );
    }

    #[test]
    fn special_passage_defs_complete() {
        let defs = special_passages::special_passage_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

        // All expected special passages
        assert!(names.contains(&"StoryInit"));
        assert!(names.contains(&"StoryTitle"));
        assert!(names.contains(&"StoryData"));
        assert!(names.contains(&"StoryCaption"));
        assert!(names.contains(&"StoryMenu"));
        assert!(names.contains(&"StoryBanner"));
        assert!(names.contains(&"StorySubtitle"));
        assert!(names.contains(&"StoryAuthor"));
        assert!(names.contains(&"StoryDisplayTitle"));
        assert!(names.contains(&"StoryShare"));
        assert!(names.contains(&"StoryInterface"));
        assert!(names.contains(&"PassageReady"));
        assert!(names.contains(&"PassageDone"));
        assert!(names.contains(&"PassageHeader"));
        assert!(names.contains(&"PassageFooter"));
        assert!(names.contains(&"Story JavaScript"));
        assert!(names.contains(&"Story Stylesheet"));
    }
}
