//! Unified Document Model
//!
//! Every file is normalized into a format-agnostic internal representation.
//! Format plugins are responsible only for parsing source text into this structure.
//! The core engine owns global graph construction, workspace indexing,
//! cross-file diagnostics, and dataflow analysis.

use crate::passage::{Passage, StoryFormat};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use url::Url;

/// A normalized, format-agnostic representation of a Twee source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// The URI of this document.
    pub uri: Url,
    /// The story format used to parse this document.
    pub format: StoryFormat,
    /// The passages contained in this document.
    pub passages: Vec<Passage>,
    /// The version number of this document (increments on each change).
    pub version: i32,
    /// An optional snapshot of the document's source text as a Rope.
    ///
    /// This is set by the server when a document is opened or changed, and
    /// used for converting byte offsets to line numbers. It is not serialized
    /// because the Rope is only needed at runtime and can be reconstructed
    /// from the source text.
    #[serde(skip)]
    pub snapshot: Option<DocumentSnapshot>,
}

impl Document {
    /// Create a new empty document.
    pub fn new(uri: Url, format: StoryFormat) -> Self {
        Self {
            uri,
            format,
            passages: Vec::new(),
            version: 0,
            snapshot: None,
        }
    }

    /// Find a passage by name.
    pub fn find_passage(&self, name: &str) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == name)
    }

    /// Find a passage by name, mutably.
    pub fn find_passage_mut(&mut self, name: &str) -> Option<&mut Passage> {
        self.passages.iter_mut().find(|p| p.name == name)
    }

    /// Find the StoryData passage, if present.
    pub fn story_data(&self) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == "StoryData")
    }

    /// Find the StoryTitle passage, if present.
    pub fn story_title(&self) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == "StoryTitle")
    }

    /// Return all links from all passages in this document.
    pub fn all_links(&self) -> impl Iterator<Item = &crate::passage::Link> {
        self.passages.iter().flat_map(|p| p.links.iter())
    }

    /// Return all variable operations from all passages.
    pub fn all_var_ops(&self) -> impl Iterator<Item = &crate::passage::VarOp> {
        self.passages.iter().flat_map(|p| p.vars.iter())
    }

    /// Increment the document version.
    ///
    /// Also updates the snapshot version if present, keeping them in sync.
    pub fn bump_version(&mut self) {
        self.version += 1;
        if let Some(ref mut snapshot) = self.snapshot {
            snapshot.version = self.version;
        }
    }

    /// Set the document snapshot from source text.
    ///
    /// Creates a `DocumentSnapshot` wrapping a Rope built from the given
    /// text and stores it in the `snapshot` field. This enables
    /// byte-offset-to-line-number conversion via [`Self::byte_to_line`].
    ///
    /// The snapshot version is synced with `self.version` so that
    /// incremental updates preserve the correct version tracking.
    pub fn set_snapshot_from_text(&mut self, text: &str) {
        let mut snapshot = DocumentSnapshot::from_document(self, text);
        snapshot.version = self.version;
        self.snapshot = Some(snapshot);
    }

    /// Convert a byte offset within this document to a 0-based line number.
    ///
    /// If the document has a snapshot (i.e., the source text is available),
    /// this uses the Rope's `byte_to_line` method for accurate conversion.
    /// Returns 0 if no snapshot is available or if the offset is out of
    /// bounds.
    pub fn byte_to_line(&self, byte_offset: usize) -> u32 {
        if let Some(ref snapshot) = self.snapshot {
            let clamped = byte_offset.min(snapshot.rope.len_bytes());
            snapshot.rope.byte_to_line(clamped) as u32
        } else {
            0
        }
    }

    /// Remove a passage by name and return it.
    pub fn remove_passage(&mut self, name: &str) -> Option<Passage> {
        let idx = self.passages.iter().position(|p| p.name == name)?;
        Some(self.passages.remove(idx))
    }

    /// Replace a passage by name, or add it if it doesn't exist.
    pub fn upsert_passage(&mut self, passage: Passage) {
        if let Some(existing) = self.find_passage_mut(&passage.name) {
            *existing = passage;
        } else {
            self.passages.push(passage);
        }
    }

    /// Apply incremental text changes to the document snapshot.
    ///
    /// Each change is specified as a `(byte_range, replacement_text)` pair.
    /// The changes are applied sequentially to the snapshot's Rope, and
    /// byte offsets in later changes refer to the text state *after* earlier
    /// changes have been applied (matching LSP incremental sync semantics).
    ///
    /// Sets `self.version = version` (the authoritative LSP version) and
    /// updates the snapshot version to match.
    ///
    /// Returns the full text after all changes have been applied, or `None`
    /// if no snapshot is available (caller should fall back to full-text sync).
    pub fn apply_incremental_change(
        &mut self,
        version: i32,
        changes: &[(std::ops::Range<usize>, String)],
    ) -> Option<String> {
        let snapshot = self.snapshot.as_mut()?;

        for (range, new_text) in changes {
            snapshot.apply_change(range.clone(), new_text);
        }

        self.version = version;
        snapshot.version = version;

        // Extract the full text from the rope after all changes
        let mut text = String::with_capacity(snapshot.rope.len_bytes());
        for chunk in snapshot.rope.chunks() {
            text.push_str(chunk);
        }
        Some(text)
    }
}

/// A snapshot of a document at a particular version, used for incremental
/// updates and change tracking.
///
/// The snapshot caches a `Rope`-based representation of the document text
/// for efficient incremental editing. When INCREMENTAL sync is active, the
/// `did_change` handler applies changes to the rope via [`apply_change`]
/// rather than replacing the entire text, then re-parses the resulting
/// full text.
#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    /// The document URI.
    pub uri: Url,
    /// The version this snapshot represents.
    pub version: i32,
    /// The full text content as a Rope for efficient incremental editing.
    pub rope: Rope,
    /// Names of passages that existed at this version.
    pub passage_names: Vec<String>,
}

impl DocumentSnapshot {
    /// Create a snapshot from a document and its text content.
    pub fn from_document(doc: &Document, text: &str) -> Self {
        Self {
            uri: doc.uri.clone(),
            version: doc.version,
            rope: Rope::from_str(text),
            passage_names: doc.passages.iter().map(|p| p.name.clone()).collect(),
        }
    }

    /// Apply a range of text changes to the rope.
    ///
    /// The `range` parameter is in **byte offsets** (from LSP position
    /// conversion). Ropey's `remove`/`insert` require **char indices**,
    /// so we convert here using `byte_to_char`.
    ///
    /// **Note:** This does NOT update `self.version`; the caller is
    /// responsible for setting the version from the authoritative LSP
    /// version number.
    pub fn apply_change(&mut self, range: Range<usize>, new_text: &str) {
        // Clamp byte offsets to the rope's byte length to avoid panics
        let byte_start = range.start.min(self.rope.len_bytes());
        let byte_end = range.end.min(self.rope.len_bytes());

        // Convert byte offsets to char indices — ropey requires char indices
        // for remove/insert, NOT byte offsets. Using byte offsets directly
        // causes panics on any document with multi-byte UTF-8 characters.
        let char_start = self.rope.byte_to_char(byte_start);
        let char_end = self.rope.byte_to_char(byte_end);

        self.rope.remove(char_start..char_end);
        self.rope.insert(char_start, new_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that apply_change correctly converts byte offsets to char indices
    /// for documents containing multi-byte UTF-8 characters.
    ///
    /// This is a regression test for the ropey panic: the old code passed
    /// byte offsets directly to rope.remove() and rope.insert(), which
    /// require char indices. With multi-byte characters (em-dash, CJK,
    /// emoji), byte offsets exceed the char count and cause ropey to panic.
    #[test]
    fn apply_change_with_multibyte_chars() {
        let mut snapshot = DocumentSnapshot {
            uri: url::Url::parse("file:///test.tw").unwrap(),
            version: 1,
            rope: Rope::from_str("Hello — World"),  // — is 3 bytes in UTF-8
            passage_names: vec![],
        };

        // The em-dash — is at byte offset 6..9 (3 bytes) but char index 6..7
        // Replace "World" (byte offset 10..15) with "Rust"
        // In char indices: "World" is at char 8..13
        snapshot.apply_change(10..15, "Rust");

        let result: String = snapshot.rope.to_string();
        assert_eq!(result, "Hello — Rust");
    }

    #[test]
    fn apply_change_at_start_with_multibyte() {
        let mut snapshot = DocumentSnapshot {
            uri: url::Url::parse("file:///test.tw").unwrap(),
            version: 1,
            rope: Rope::from_str("日本語テスト"),  // CJK characters (3 bytes each)
            passage_names: vec![],
        };

        // Replace first 3 chars (byte 0..9) with "Hello"
        snapshot.apply_change(0..9, "Hello");

        let result: String = snapshot.rope.to_string();
        assert_eq!(result, "Helloテスト");
    }

    #[test]
    fn apply_change_insert_in_multibyte() {
        let mut snapshot = DocumentSnapshot {
            uri: url::Url::parse("file:///test.tw").unwrap(),
            version: 1,
            rope: Rope::from_str("a—b"),  // 1 + 3 + 1 = 5 bytes, 3 chars
            passage_names: vec![],
        };

        // Insert "XX" after the em-dash (byte offset 4, char index 2)
        snapshot.apply_change(4..4, "XX");

        let result: String = snapshot.rope.to_string();
        assert_eq!(result, "a—XXb");
    }

    #[test]
    fn apply_change_with_emoji() {
        let mut snapshot = DocumentSnapshot {
            uri: url::Url::parse("file:///test.tw").unwrap(),
            version: 1,
            rope: Rope::from_str("Hello 🌍 World"),  // 🌍 is 4 bytes
            passage_names: vec![],
        };

        // Replace "World" (after emoji) with "Earth"
        // "Hello " = 6 bytes, "🌍" = 4 bytes, " " = 1 byte
        // "World" starts at byte 11, ends at byte 16
        snapshot.apply_change(11..16, "Earth");

        let result: String = snapshot.rope.to_string();
        assert_eq!(result, "Hello 🌍 Earth");
    }

    #[test]
    fn apply_change_clamps_out_of_bounds() {
        let mut snapshot = DocumentSnapshot {
            uri: url::Url::parse("file:///test.tw").unwrap(),
            version: 1,
            rope: Rope::from_str("abc"),
            passage_names: vec![],
        };

        // Out-of-bounds byte range should be clamped, not panic
        snapshot.apply_change(0..1000, "xyz");

        let result: String = snapshot.rope.to_string();
        assert_eq!(result, "xyz");
    }
}
