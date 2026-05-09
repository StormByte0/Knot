//! Special passage definitions for SugarCube.
//!
//! Contains the comprehensive list of SugarCube special passage definitions,
//! including lifecycle passages (StoryInit, PassageReady), chrome passages
//! (StoryCaption, StoryBanner, etc.), metadata passages, and system passages
//! (Story JavaScript, Story Stylesheet).

use knot_core::passage::{SpecialPassageBehavior, SpecialPassageDef};

/// SugarCube special passage definitions.
///
/// These define the passages that have special meaning in SugarCube 2.x,
/// including their behavior, whether they contribute variables, and their
/// execution priority.
pub(crate) fn special_passage_defs() -> Vec<SpecialPassageDef> {
    vec![
        // ── Lifecycle passages ─────────────────────────────────────────
        SpecialPassageDef {
            name: "StoryInit".into(),
            behavior: SpecialPassageBehavior::Startup,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(0),
        },
        SpecialPassageDef {
            name: "PassageReady".into(),
            behavior: SpecialPassageBehavior::PassageReady,
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(50),
        },
        SpecialPassageDef {
            name: "PassageDone".into(),
            behavior: SpecialPassageBehavior::Custom("PassageDone".into()),
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(200),
        },

        // ── Chrome passages ────────────────────────────────────────────
        SpecialPassageDef {
            name: "PassageHeader".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Prepended to each rendered passage
            execution_priority: Some(90),
        },
        SpecialPassageDef {
            name: "PassageFooter".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Appended to each rendered passage
            execution_priority: Some(110),
        },
        SpecialPassageDef {
            name: "StoryCaption".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(100),
        },
        SpecialPassageDef {
            name: "StoryMenu".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(101),
        },
        SpecialPassageDef {
            name: "StoryBanner".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(102),
        },
        SpecialPassageDef {
            name: "StorySubtitle".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(103),
        },
        SpecialPassageDef {
            name: "StoryAuthor".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(104),
        },
        SpecialPassageDef {
            name: "StoryDisplayTitle".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(105),
        },
        SpecialPassageDef {
            name: "StoryShare".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(106),
        },
        SpecialPassageDef {
            name: "StoryInterface".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(107),
        },

        // ── Metadata passages ──────────────────────────────────────────
        SpecialPassageDef {
            name: "StoryTitle".into(),
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
        },
        SpecialPassageDef {
            name: "StoryData".into(),
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
        },

        // ── System passages (script/stylesheet) ────────────────────────
        SpecialPassageDef {
            name: "Story JavaScript".into(),
            behavior: SpecialPassageBehavior::Custom("StoryJavaScript".into()),
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(10),
        },
        SpecialPassageDef {
            name: "Story Stylesheet".into(),
            behavior: SpecialPassageBehavior::Custom("StoryStylesheet".into()),
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(11),
        },
    ]
}
