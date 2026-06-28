//! Passage classifier — two-pass detect + classify system.
//!
//! The classifier implements the Twee 3 spec rule: **tags override names**.
//! This means a passage named "StoryInit" but tagged `[script]` is classified
//! as a script passage, NOT as StoryInit.
//!
//! ## Classification Priority
//!
//! 1. **Core name-matched** — StoryTitle, StoryData, Start (always recognized)
//! 2. **Core tag-matched** — [script], [stylesheet], [style] (Twine compiler)
//! 3. **Format tag-matched** — [init], [widget] (SugarCube-specific)
//! 4. **Format name-matched** — StoryInit, PassageHeader, etc.
//! 5. **Normal passage** — user-defined, with or without custom tags
//!
//! ## Processing Order (separate from classification)
//!
//! Classification determines WHAT a passage is. Processing order determines
//! WHEN it gets parsed (define-before-use):
//!
//! 1. `[script]` passages → oxc → warm variable/macro registries
//! 2. `[widget]` passages → SugarCube parser → warm widget registry
//! 3. Named specials → SugarCube parser (registries warm)
//! 4. Normal passages → SugarCube parser (all registries available)
//! 5. Stylesheets/StoryData → skip or minimal
//!
//! ## Data Flow
//!
//! ```text
//! Vec<(TweeHeader, &str)>    ← from lexer::split_passages()
//!         |
//!         v
//! classify_all()              ← produces Vec<ClassifiedPassage>
//!         |
//!         v
//! sort_for_processing()       ← reorders by processing priority
//!         |
//!         v
//! parser::parse_passage()     ← processes each in order
//! ```

use super::special_passages;
use crate::header::TweeHeader;
use knot_core::passage::{MatchStrategy, SpecialPassageBehavior, SpecialPassageDef};

// ---------------------------------------------------------------------------
// ClassifiedPassage
// ---------------------------------------------------------------------------

/// A passage that has been classified but not yet parsed.
///
/// Carries the header info, body text, classification result, and
/// the processing priority that determines parse order.
#[derive(Debug, Clone)]
pub struct ClassifiedPassage {
    /// The parsed header (name, tags, position, metadata).
    pub header: TweeHeader,
    /// The raw body text (reference not possible — we own it after classification).
    pub body_text: String,
    /// The file URI this passage belongs to.
    pub file_uri: String,
    /// Classification result: Some(def) if special, None if normal.
    pub special_def: Option<SpecialPassageDef>,
    /// The passage category from classification.
    pub category: PassageCategory,
    /// Processing priority (lower = parsed first).
    /// See PROCESSING_* constants below.
    pub processing_priority: u8,
}

// ---------------------------------------------------------------------------
// PassageCategory
// ---------------------------------------------------------------------------

/// The classification category of a passage.
///
/// This is the result of the tags-first priority check. Each category
/// maps to a processing priority that determines parse order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassageCategory {
    /// StoryData, StoryTitle (core metadata — format detection)
    CoreMetadata,
    /// Start (core name-matched non-metadata)
    CoreNamed,
    /// [script], [stylesheet], [style] (core tags)
    CoreTagged,
    /// "script"/"stylesheet" as names (Twine 1 legacy)
    CoreLegacy,
    /// [init], [widget] (format tags)
    FormatTagged,
    /// StoryInit, PassageHeader, etc. (format names)
    FormatNamed,
    /// Normal user-defined passage
    Regular,
}

// ---------------------------------------------------------------------------
// Processing priority constants
// ---------------------------------------------------------------------------

/// Process script passages first to warm variable/macro registries.
pub const PROCESSING_SCRIPT: u8 = 10;
/// Process widget passages second to warm widget registry.
pub const PROCESSING_WIDGET: u8 = 20;
/// Process named special passages third (registries now warm).
pub const PROCESSING_NAMED_SPECIAL: u8 = 30;
/// Process normal passages fourth (can query all registries).
pub const PROCESSING_NORMAL: u8 = 40;
/// Process stylesheets and StoryData last (minimal processing).
pub const PROCESSING_STYLESHEET: u8 = 50;
/// Core metadata passages (StoryData) — minimal processing.
pub const PROCESSING_METADATA: u8 = 50;

// ---------------------------------------------------------------------------
// classify_all()
// ---------------------------------------------------------------------------

/// Classify all passages from a file.
///
/// Takes the raw (header, body) pairs from `lexer::split_passages()`
/// and produces `ClassifiedPassage` entries with classification and
/// processing priority set.
///
/// The classification uses the `FormatPlugin::classify_passage_category()`
/// logic directly (tags-first priority), but since we don't have a
/// FormatPlugin reference here, we implement the same logic locally
/// using the known special passage definitions.
pub fn classify_all(raw_passages: &[(TweeHeader, &str)], file_uri: &str) -> Vec<ClassifiedPassage> {
    // Collect all applicable definitions
    let core_name_defs = knot_core::passage::twine_core_special_passages();
    let legacy_name_defs = knot_core::passage::legacy_core_special_passages();
    let format_name_defs = special_passages::name_matched_special_passages();
    let format_tag_defs = special_passages::tag_matched_special_passages();

    let mut results = Vec::with_capacity(raw_passages.len());

    for (header, body) in raw_passages {
        let (special_def, category) = classify_passage(
            &header.name,
            &header.tags,
            &core_name_defs,
            &legacy_name_defs,
            &format_name_defs,
            &format_tag_defs,
        );

        let processing_priority = compute_processing_priority(&category, &special_def);

        results.push(ClassifiedPassage {
            header: header.clone(),
            body_text: body.to_string(),
            file_uri: file_uri.to_string(),
            special_def,
            category,
            processing_priority,
        });
    }

    results
}

// ---------------------------------------------------------------------------
// sort_for_processing()
// ---------------------------------------------------------------------------

/// Sort classified passages by processing priority (define-before-use).
///
/// Script passages are processed first (to warm registries), then widgets,
/// then named specials, then normal passages, then stylesheets/metadata.
/// Within the same priority level, passages retain their source order.
pub fn sort_for_processing(passages: &mut [ClassifiedPassage]) {
    passages.sort_by_key(|p| p.processing_priority);
}

// ---------------------------------------------------------------------------
// classify_passage()
// ---------------------------------------------------------------------------

/// Classify a single passage against all known definitions.
///
/// Implements the tags-first priority per the Twee 3 spec:
/// 1. Core name-matched (StoryTitle, StoryData, Start)
/// 2. Core tag-matched ([script], [stylesheet], [style])
/// 3. Format tag-matched ([init], [widget])
/// 4. Format name-matched (StoryInit, PassageHeader, etc.)
/// 5. Regular (no match)
fn classify_passage(
    passage_name: &str,
    passage_tags: &[String],
    core_name_defs: &[SpecialPassageDef],
    legacy_name_defs: &[SpecialPassageDef],
    format_name_defs: &[SpecialPassageDef],
    format_tag_defs: &[SpecialPassageDef],
) -> (Option<SpecialPassageDef>, PassageCategory) {
    // Step 1: Core name-matched (HIGHEST PRIORITY)
    for def in core_name_defs {
        if def.match_strategy == MatchStrategy::Name && def.name == passage_name {
            let category = if matches!(def.behavior, SpecialPassageBehavior::Metadata) {
                PassageCategory::CoreMetadata
            } else {
                PassageCategory::CoreNamed
            };
            return (Some(def.clone()), category);
        }
    }

    // Step 2: Legacy name-matched (Twine 1 compat)
    for def in legacy_name_defs {
        if def.match_strategy == MatchStrategy::Name && def.name == passage_name {
            return (Some(def.clone()), PassageCategory::CoreLegacy);
        }
    }

    // Step 3: Core tag-matched ([script], [stylesheet], [style])
    for tag in passage_tags {
        for def in core_name_defs {
            if def.match_strategy == MatchStrategy::Tag && tag.eq_ignore_ascii_case(&def.name) {
                let mut matched = def.clone();
                matched.name = passage_name.to_string();
                return (Some(matched), PassageCategory::CoreTagged);
            }
        }
        for def in legacy_name_defs {
            if def.match_strategy == MatchStrategy::Tag && tag.eq_ignore_ascii_case(&def.name) {
                let mut matched = def.clone();
                matched.name = passage_name.to_string();
                return (Some(matched), PassageCategory::CoreTagged);
            }
        }
    }

    // Step 4: Format tag-matched ([init], [widget])
    for tag in passage_tags {
        for def in format_tag_defs {
            if tag.eq_ignore_ascii_case(&def.name) {
                let mut matched = def.clone();
                matched.name = passage_name.to_string();
                return (Some(matched), PassageCategory::FormatTagged);
            }
        }
    }

    // Step 5: Format name-matched (StoryInit, PassageHeader, etc.)
    for def in format_name_defs {
        if def.name == passage_name {
            return (Some(def.clone()), PassageCategory::FormatNamed);
        }
    }

    (None, PassageCategory::Regular)
}

// ---------------------------------------------------------------------------
// compute_processing_priority()
// ---------------------------------------------------------------------------

/// Determine the processing priority for a classified passage.
///
/// The priority determines parse order: lower values are parsed first.
/// This implements the define-before-use principle:
///
/// 1. Script passages → warm variable/macro registries
/// 2. Widget passages → warm widget registry
/// 3. Named specials → parse with warm registries
/// 4. Normal passages → parse with all registries available
/// 5. Stylesheets/metadata → minimal processing
fn compute_processing_priority(
    category: &PassageCategory,
    special_def: &Option<SpecialPassageDef>,
) -> u8 {
    match category {
        PassageCategory::CoreMetadata => PROCESSING_METADATA,
        PassageCategory::CoreTagged => {
            // Check if this is a script or stylesheet
            if let Some(def) = special_def {
                match def.name.as_str() {
                    "script" | "Story JavaScript" => PROCESSING_SCRIPT,
                    "stylesheet" | "style" | "Story Stylesheet" => PROCESSING_STYLESHEET,
                    _ => PROCESSING_NAMED_SPECIAL,
                }
            } else {
                PROCESSING_NAMED_SPECIAL
            }
        }
        PassageCategory::CoreLegacy => {
            if let Some(def) = special_def {
                match def.name.as_str() {
                    "script" => PROCESSING_SCRIPT,
                    "stylesheet" => PROCESSING_STYLESHEET,
                    _ => PROCESSING_NAMED_SPECIAL,
                }
            } else {
                PROCESSING_NAMED_SPECIAL
            }
        }
        PassageCategory::FormatTagged => {
            // [widget] → parse early; [init] → parse with scripts
            if let Some(def) = special_def {
                if def.name.eq_ignore_ascii_case("widget") {
                    PROCESSING_WIDGET
                } else if def.name.eq_ignore_ascii_case("init") {
                    PROCESSING_SCRIPT // init runs at startup, parse early
                } else {
                    PROCESSING_NAMED_SPECIAL
                }
            } else {
                PROCESSING_NAMED_SPECIAL
            }
        }
        PassageCategory::CoreNamed => PROCESSING_NAMED_SPECIAL,
        PassageCategory::FormatNamed => {
            // StoryInit, PassageHeader, etc. — parse after scripts/widgets
            if let Some(def) = special_def {
                if matches!(def.behavior, SpecialPassageBehavior::Startup) {
                    PROCESSING_SCRIPT // startup passages warm registries
                } else {
                    PROCESSING_NAMED_SPECIAL
                }
            } else {
                PROCESSING_NAMED_SPECIAL
            }
        }
        PassageCategory::Regular => {
            // Check if it's a script passage by tag (shouldn't happen after
            // classification, but be safe)
            PROCESSING_NORMAL
        }
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Check if a classified passage is a script passage (should be parsed by oxc).
pub fn is_script_passage(cp: &ClassifiedPassage) -> bool {
    matches!(
        cp.category,
        PassageCategory::CoreTagged | PassageCategory::CoreLegacy
    ) && cp
        .special_def
        .as_ref()
        .is_some_and(|d| d.name.eq_ignore_ascii_case("script") || d.name == "Story JavaScript")
        || cp
            .header
            .tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case("script"))
        || cp.header.name == "Story JavaScript"
}

/// Check if a classified passage is a stylesheet passage (minimal processing).
pub fn is_stylesheet_passage(cp: &ClassifiedPassage) -> bool {
    matches!(
        cp.category,
        PassageCategory::CoreTagged | PassageCategory::CoreLegacy
    ) && cp.special_def.as_ref().is_some_and(|d| {
        d.name.eq_ignore_ascii_case("stylesheet")
            || d.name.eq_ignore_ascii_case("style")
            || d.name == "Story Stylesheet"
    }) || cp
        .header
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("stylesheet") || t.eq_ignore_ascii_case("style"))
        || cp.header.name == "Story Stylesheet"
}

/// Check if a classified passage is a widget passage.
pub fn is_widget_passage(cp: &ClassifiedPassage) -> bool {
    matches!(cp.category, PassageCategory::FormatTagged)
        && cp
            .special_def
            .as_ref()
            .is_some_and(|d| d.name.eq_ignore_ascii_case("widget"))
        || cp
            .header
            .tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case("widget"))
}

/// Check if a classified passage is the StoryInterface passage.
pub fn is_interface_passage(cp: &ClassifiedPassage) -> bool {
    cp.header.name == "StoryInterface"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(name: &str, tags: Vec<&str>) -> TweeHeader {
        TweeHeader {
            name: name.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            header_start: 0,
            name_start: 3,
            metadata_json: None,
            name_text_raw: name.to_string(),
            tags_raw: String::new(),
        }
    }

    #[test]
    fn classify_core_name_matched() {
        let (def, cat) = classify_passage(
            "StoryTitle",
            &[],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_some());
        assert_eq!(cat, PassageCategory::CoreMetadata);
    }

    #[test]
    fn classify_core_tag_script() {
        let (def, cat) = classify_passage(
            "MyScript",
            &["script".to_string()],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_some());
        assert_eq!(cat, PassageCategory::CoreTagged);
    }

    #[test]
    fn classify_format_tag_widget() {
        let (def, cat) = classify_passage(
            "MyWidget",
            &["widget".to_string()],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_some());
        assert_eq!(cat, PassageCategory::FormatTagged);
    }

    #[test]
    fn classify_format_name_storyinit() {
        let (def, cat) = classify_passage(
            "StoryInit",
            &[],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_some());
        assert_eq!(cat, PassageCategory::FormatNamed);
    }

    #[test]
    fn classify_regular_passage() {
        let (def, cat) = classify_passage(
            "Forest",
            &["dark".to_string()],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_none());
        assert_eq!(cat, PassageCategory::Regular);
    }

    #[test]
    fn tags_override_names() {
        // A passage named "StoryInit" but tagged [script] is a SCRIPT passage,
        // not StoryInit. Tags take priority per Twee 3 spec.
        let (def, cat) = classify_passage(
            "StoryInit",
            &["script".to_string()],
            &knot_core::passage::twine_core_special_passages(),
            &knot_core::passage::legacy_core_special_passages(),
            &special_passages::name_matched_special_passages(),
            &special_passages::tag_matched_special_passages(),
        );
        assert!(def.is_some());
        // Should be CoreTagged (script), not FormatNamed (StoryInit)
        assert_eq!(cat, PassageCategory::CoreTagged);
    }

    #[test]
    fn processing_order_scripts_first() {
        let mut passages = vec![
            ClassifiedPassage {
                header: make_header("Forest", vec![]),
                body_text: "Normal passage".into(),
                file_uri: "file:///test.tw".into(),
                special_def: None,
                category: PassageCategory::Regular,
                processing_priority: PROCESSING_NORMAL,
            },
            ClassifiedPassage {
                header: make_header("MyScript", vec!["script"]),
                body_text: "Script code".into(),
                file_uri: "file:///test.tw".into(),
                special_def: None,
                category: PassageCategory::CoreTagged,
                processing_priority: PROCESSING_SCRIPT,
            },
        ];

        sort_for_processing(&mut passages);
        assert_eq!(passages[0].header.name, "MyScript"); // Script first
        assert_eq!(passages[1].header.name, "Forest"); // Normal second
    }
}
