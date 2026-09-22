//! Special passage and tag definitions for SugarCube.
//!
//! Contains the comprehensive list of SugarCube 2.x special passage definitions,
//! organized by matching strategy:
//!
//! - **Name-matched** code passages (StoryInit, PassageHeader, etc.)
//! - **Tag-matched** code tags ([init], [widget])
//!
//! **Note:** StoryTitle, StoryData, Start, [script], [stylesheet], and [style]
//! are NOT defined here — they are Twine-core passages defined in
//! `knot_core::passage::twine_core_special_passages()`. The [style] tag is a
//! Twee 3 / Tweego alias for [stylesheet] and is recognized at the core level.
//!
//! ## Format Isolation
//!
//! SugarCube's code passages (StoryInit, PassageHeader, etc.) are **name-matched**
//! — the passage NAME must exactly match. This differs from Harlowe, where
//! equivalent functionality is **tag-matched** (e.g., [header], [footer],
//! [startup]). The `MatchStrategy` field on each `SpecialPassageDef` encodes
//! this distinction so the classification system handles both correctly.
//!
//! ## Script & Stylesheet Passages
//!
//! [script] and [stylesheet] are **core** special tags defined by the Twee 3
//! specification, not SugarCube-specific tags. SugarCube AUGMENTS them with
//! additional behaviors (e.g., "cannot be navigated to"), but does not own them.
//! The augmentation is handled by the classification system, which merges core
//! tag definitions with format-specific behaviors.
//!
//! ## SugarCube-Specific Tags
//!
//! SugarCube defines additional code tags beyond the core:
//!
//! - `[init]` — Initialization tag (SugarCube 2.36+). Equivalent to StoryInit
//!   but tag-based, intended for add-ons/libraries.
//! - `[widget]` — Widget definition tag. Passages tagged [widget] define
//!   reusable macros.
//!
//! Note: `[nobr]` is NOT a special tag — it is a rendering hint that strips
//! line breaks from passage output. Passages with `[nobr]` are still normal
//! navigable passages and should not be classified as special passages.
//!
//! ## Case Sensitivity
//!
//! All SugarCube name-matched passage names are **case-sensitive**. The spelling
//! and capitalization must be exactly as shown (e.g., `StoryInit`, not
//! `storyinit` or `Story Init`). Tag matching is case-insensitive per the
//! Twee 3 spec.

use knot_core::passage::{
    MatchStrategy, ScaffoldInfo, SpecialPassageBehavior, SpecialPassageDef, SpecialPassageLayer,
};

/// SugarCube name-matched special passage definitions.
///
/// These are code passages identified by their exact passage name.
/// All names are case-sensitive — must match exactly as shown.
///
/// **Format isolation**: These definitions are SugarCube-specific.
/// Harlowe achieves the same functional results through tag-matched
/// passages (e.g., [header] instead of PassageHeader, [startup]
/// instead of StoryInit).
pub(crate) fn name_matched_special_passages() -> Vec<SpecialPassageDef> {
    vec![
        // ── Lifecycle passages ─────────────────────────────────────────
        SpecialPassageDef {
            name: "StoryInit".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Startup,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(0),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: Some(ScaffoldInfo {
                file_name: "_format_special_passages.twee".into(),
                default_passage_name: "StoryInit".into(),
                default_content: String::new(),
            }),
        },
        SpecialPassageDef {
            name: "PassageReady".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::PassageReady,
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(50),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "PassageDone".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Custom("PassageDone".into()),
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(200),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        // ── Chrome interceptor passages ────────────────────────────────
        // PassageHeader and PassageFooter are rendering interceptors: they
        // are prepended/appended to every rendered passage body. They wrap
        // every user-defined passage during rendering but are NOT navigation
        // targets. The graph does NOT create O(N) edges from interceptors
        // to every user passage; instead, the analysis engine treats them
        // as always-invoked at render time. Their variable context is merged
        // into every passage's entry state during dataflow analysis.
        SpecialPassageDef {
            name: "PassageHeader".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::ChromeInterceptor,
            contributes_variables: true, // Can set/modify variables visible in passage body
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(90),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "PassageFooter".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::ChromeInterceptor,
            contributes_variables: true, // Can set/modify variables visible in next passage
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(110),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryCaption".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(100),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryMenu".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(101),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryBanner".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(102),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StorySubtitle".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(103),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryAuthor".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(104),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryDisplayTitle".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(105),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryShare".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(106),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "StoryInterface".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::StructureTemplate,
            contributes_variables: false,
            participates_in_graph: true, // Contains data-passage refs to user passages
            execution_priority: Some(107),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
    ]
}

/// SugarCube tag-matched special passage definitions.
///
/// These are code tags and special tags identified by their tag name
/// in the passage header (e.g., `:: MyWidget [widget]`).
/// Tag matching is case-insensitive per the Twee 3 spec.
///
/// **Format isolation**: These tags are SugarCube-specific.
/// Harlowe uses different tag names for different purposes
/// (e.g., [header], [footer], [startup] instead of [init], [widget]).
///
/// ## Core Tags NOT Repeated Here
///
/// [script] and [stylesheet] are **core** tags defined by the Twee 3
/// specification. They are NOT repeated here. SugarCube augments them
/// with additional behaviors (e.g., "cannot be navigated to"), but the
/// core definitions live in `twine_core_special_passages()`.
pub(crate) fn tag_matched_special_passages() -> Vec<SpecialPassageDef> {
    vec![
        // ── Code tags ──────────────────────────────────────────────────
        // [init] — SugarCube 2.36+. Registers the passage as an
        // initialization passage for pre-story-start tasks. Primarily
        // intended for add-ons/libraries; normal projects should use
        // the StoryInit named passage.
        SpecialPassageDef {
            name: "init".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::Startup,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(0), // Same priority as StoryInit
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        // [widget] — Widget definition tag. Passages tagged [widget]
        // define reusable custom macros. They cannot be navigated to.
        SpecialPassageDef {
            name: "widget".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::Custom("Widget".into()),
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        },
        // NOTE: [nobr] is NOT listed here because it is a rendering hint,
        // not a special passage classification. Passages tagged [nobr] are
        // normal navigable passages — the tag only strips line breaks from
        // the rendered output. Treating it as a "special passage" would
        // incorrectly give it SpecialPassage semantic tokens (different
        // highlighting) and potentially exclude it from graph analysis.
        //
        // If [nobr] is ever needed for diagnostics or graph edges, it should
        // be handled as a passage property, not as a special passage category.
    ]
}

// ── Version filtering ───────────────────────────────────────────────────────

/// Special passage/tag names introduced after the SugarCube 2.0.0 baseline,
/// with the version that introduced them. Anything not listed here has
/// existed since 2.0.0.
///
/// Sources: SugarCube v2 docs per-passage History sections.
const POST_2_0_ADDITIONS: &[(&str, (u32, u32, u32))] =
    &[("StoryDisplayTitle", (2, 31, 0)), ("init", (2, 36, 0))];

/// Whether a special passage/tag (by name) exists at the given SugarCube
/// version.
///
/// Tag names match case-insensitively in the engine; this check is
/// case-sensitive like the defs themselves — callers pass the def's own
/// `name`, so both spellings agree.
pub(crate) fn exists_at_version(name: &str, version: crate::types::FormatVersion) -> bool {
    let added = POST_2_0_ADDITIONS.iter().find(|(n, _)| *n == name).map_or(
        crate::types::FormatVersion::new(2, 0, 0),
        |(_, (major, minor, patch))| crate::types::FormatVersion::new(*major, *minor, *patch),
    );
    version >= added
}

#[cfg(test)]
mod tests {
    use crate::plugin::FormatPlugin;
    use crate::sugarcube::SugarCubePlugin;
    use crate::types::FormatVersion;

    fn special_names(plugin: &SugarCubePlugin) -> Vec<String> {
        plugin
            .all_special_passages()
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }

    #[test]
    fn special_passages_are_version_filtered() {
        // Default (no pinned version) = latest: StoryDisplayTitle + [init]
        // are both known.
        let plugin = SugarCubePlugin::new();
        let names = special_names(&plugin);
        assert!(names.iter().any(|n| n == "StoryDisplayTitle"));
        assert!(names.iter().any(|n| n == "init"));

        // Before 2.31.0, StoryDisplayTitle doesn't exist.
        let plugin = SugarCubePlugin::new();
        plugin.set_story_version(Some(FormatVersion::new(2, 30, 0)));
        let names = special_names(&plugin);
        assert!(
            !names.iter().any(|n| n == "StoryDisplayTitle"),
            "StoryDisplayTitle was added in 2.31.0"
        );
        assert!(
            !names.iter().any(|n| n == "init"),
            "[init] tag was added in 2.36.0"
        );

        // At 2.31.0 exactly, StoryDisplayTitle exists but [init] doesn't.
        let plugin = SugarCubePlugin::new();
        plugin.set_story_version(Some(FormatVersion::new(2, 31, 0)));
        let names = special_names(&plugin);
        assert!(names.iter().any(|n| n == "StoryDisplayTitle"));
        assert!(!names.iter().any(|n| n == "init"));

        // At 2.36.0, [init] exists too.
        let plugin = SugarCubePlugin::new();
        plugin.set_story_version(Some(FormatVersion::new(2, 36, 0)));
        let names = special_names(&plugin);
        assert!(names.iter().any(|n| n == "init"));

        // Ancient passages exist at every 2.x version.
        let plugin = SugarCubePlugin::new();
        plugin.set_story_version(Some(FormatVersion::new(2, 0, 0)));
        let names = special_names(&plugin);
        assert!(names.iter().any(|n| n == "StoryInit"));
        assert!(names.iter().any(|n| n == "PassageHeader"));
    }
}
