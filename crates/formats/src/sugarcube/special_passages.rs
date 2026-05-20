//! Special passage definitions for SugarCube.
//!
//! Contains the comprehensive list of SugarCube 2.x special passage definitions,
//! including lifecycle passages (StoryInit, PassageReady), chrome passages
//! (StoryCaption, StoryBanner, etc.).
//!
//! **Note:** StoryTitle and StoryData are NOT defined here — they are
//! Twine-core passages defined in `knot_core::passage::twine_core_special_passages()`.
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

use knot_core::passage::{SpecialPassageBehavior, SpecialPassageDef, SpecialPassageLayer};

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
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "PassageReady".into(),
            behavior: SpecialPassageBehavior::PassageReady,
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(50),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "PassageDone".into(),
            behavior: SpecialPassageBehavior::Custom("PassageDone".into()),
            contributes_variables: true,
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(200),
            layer: SpecialPassageLayer::StoryFormat,
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
            behavior: SpecialPassageBehavior::ChromeInterceptor,
            contributes_variables: true, // Can set/modify variables visible in passage body
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(90),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "PassageFooter".into(),
            behavior: SpecialPassageBehavior::ChromeInterceptor,
            contributes_variables: true, // Can set/modify variables visible in next passage
            participates_in_graph: true, // Invoked on every navigation
            execution_priority: Some(110),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryCaption".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(100),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryMenu".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(101),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryBanner".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: true, // Updated on every navigation
            execution_priority: Some(102),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StorySubtitle".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(103),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryAuthor".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(104),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryDisplayTitle".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(105),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryShare".into(),
            behavior: SpecialPassageBehavior::Chrome,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: Some(106),
            layer: SpecialPassageLayer::StoryFormat,
        },
        SpecialPassageDef {
            name: "StoryInterface".into(),
            behavior: SpecialPassageBehavior::StructureTemplate,
            contributes_variables: false,
            participates_in_graph: true, // Contains data-passage refs to user passages
            execution_priority: Some(107),
            layer: SpecialPassageLayer::StoryFormat,
        },

        // NOTE: StoryTitle and StoryData have been moved to TwineCore
        // (see `knot_core::passage::twine_core_special_passages()`).

        // NOTE: "Story JavaScript" and "Story Stylesheet" are NOT included
        // here. See the module-level documentation for the reasoning.
        // Script/stylesheet passages are detected via [script]/[stylesheet]
        // tags in the passage header, not by passage name.
    ]
}
