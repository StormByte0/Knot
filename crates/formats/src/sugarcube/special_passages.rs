//! Special passage definitions for SugarCube.
//!
//! Contains the comprehensive list of SugarCube 2.x special passage definitions,
//! including lifecycle passages (StoryInit, PassageReady), chrome passages
//! (StoryCaption, StoryBanner, etc.), and metadata passages.
//!
//! ## Script & Stylesheet Passages
//!
//! "Story JavaScript" and "Story Stylesheet" are **not** SugarCube engine
//! special passage names. They are Twine 2 editor/compiler concepts. In the
//! compiled HTML, they become `<script>` and `<style>` children of
//! `<tw-storydata>`, not named passages in SugarCube's passage store.
//!
//! SugarCube loads them internally as `tw-user-script-0` and
//! `tw-user-style-0` — they never appear as passage names in the engine.
//!
//! In Twee source files, script and stylesheet passages are identified by
//! their **tags** (`[script]` or `[stylesheet]`), not by their passage name.
//! The passage name can be anything (e.g., `:: MyScript[script]` works fine).
//!
//! Therefore, we do NOT include "Story JavaScript" or "Story Stylesheet"
//! in the special passage definitions. The `is_script_passage()` and
//! `is_stylesheet_passage()` methods on `Passage` handle tag-based detection
//! instead.
//!
//! ## Case Sensitivity
//!
//! All SugarCube special passage names are **case-sensitive**. The spelling
//! and capitalization must be exactly as shown (e.g., `StoryInit`, not
//! `storyinit` or `Story Init`). The name comparison in this module uses
//! exact string matching (`==`), which is correct.

use knot_core::passage::{SpecialPassageBehavior, SpecialPassageDef};

/// SugarCube special passage definitions.
///
/// These define the passages that have special meaning in SugarCube 2.x,
/// including their behavior, whether they contribute variables, and their
/// execution priority.
///
/// All names are case-sensitive — must match exactly as shown.
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

        // NOTE: "Story JavaScript" and "Story Stylesheet" are NOT included
        // here. See the module-level documentation for the reasoning.
        // Script/stylesheet passages are detected via [script]/[stylesheet]
        // tags in the passage header, not by passage name.
    ]
}
