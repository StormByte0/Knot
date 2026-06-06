//! Special/system passage names and implicit passage reference patterns.
//!
//! Self-contained leaf module providing the canonical lists of SugarCube
//! special passage names, system passage names, and patterns for detecting
//! implicit passage references in raw text/HTML/JS.

use std::collections::HashSet;

use crate::types::ImplicitPassagePattern;

/// Special/lifecycle passage names in SugarCube.
///
/// All names are case-sensitive — must match exactly as shown.
/// "Story JavaScript" and "Story Stylesheet" are NOT included because they
/// are Twine 2 editor concepts, not SugarCube engine passage names.
/// Script/stylesheet passages are identified by [script]/[stylesheet] tags.
pub fn special_passage_names() -> HashSet<&'static str> {
    [
        "StoryInit", "StoryCaption", "StoryBanner", "StorySubtitle",
        "StoryAuthor", "StoryMenu", "StoryDisplayTitle", "StoryShare",
        "StoryInterface",
        "PassageDone", "PassageHeader", "PassageFooter", "PassageReady",
        "StoryTitle", "StoryData",
    ]
    .into_iter()
    .collect()
}

/// System passages that are always reachable regardless of link structure.
///
/// Only includes metadata passages (StoryData, StoryTitle) since they are
/// data containers. Script/stylesheet passages are detected via tags, not names.
pub fn system_passage_names() -> HashSet<&'static str> {
    ["StoryData", "StoryTitle"]
        .into_iter()
        .collect()
}

/// Patterns that detect passage references in raw text / HTML / JS.
///
/// These are SugarCube-specific patterns for detecting implicit passage
/// references that are not standard `[[links]]` or `<<macro>>` passage-args,
/// such as `data-passage` attributes and `Engine.play()` calls.
pub fn implicit_passage_patterns() -> Vec<ImplicitPassagePattern> {
    vec![
        ImplicitPassagePattern {
            pattern: r#"data-passage\s*=\s*["']([^"']+)["'"#,
            description: "data-passage attribute",
        },
        ImplicitPassagePattern {
            pattern: r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["'"#,
            description: "Engine.play() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["'"#,
            description: "Engine.goto() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["'"#,
            description: "Story.get() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["'"#,
            description: "Story.passage() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Story\s*\.\s*has\s*\(\s*["']([^"']+)["'"#,
            description: "Story.has() call",
        },
        ImplicitPassagePattern {
            pattern: r#"UI\s*\.\s*goto\s*\(\s*["']([^"']+)["'"#,
            description: "UI.goto() call",
        },
        ImplicitPassagePattern {
            pattern: r#"UI\s*\.\s*include\s*\(\s*["']([^"']+)["'"#,
            description: "UI.include() call",
        },
    ]
}
