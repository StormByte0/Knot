//! URI normalization.

use url::Url;

/// Normalize a file:// URI to a canonical form.
///
/// **Root cause of duplicate passage errors**: On Windows, `Url::from_file_path()`
/// produces URIs like `file:///d:/path` (unencoded colon in the drive letter),
/// while VS Code sends URIs like `file:///d%3A/path` (colon percent-encoded).
/// These are semantically equivalent but have different serializations, causing
/// `HashMap<Url, _>` to treat them as different keys. The same file then gets
/// stored twice — once from workspace indexing and once from `did_open` — each
/// containing the same passages, which triggers duplicate passage name errors.
///
/// The fix: convert file URIs to a file path and back, which produces a
/// consistent serialization regardless of the input encoding. Non-file URIs
/// are returned unchanged.
pub(crate) fn normalize_file_uri(uri: &Url) -> Url {
    // Only normalize file:// URIs
    if uri.scheme() != "file" {
        return uri.clone();
    }

    // Try to convert to a file path and back. This produces a consistent
    // URI encoding (e.g., `file:///d:/path` on Windows) regardless of
    // whether the input had percent-encoded colons (`%3A`) or not.
    match uri.to_file_path() {
        Ok(path) => match Url::from_file_path(&path) {
            Ok(normalized) => normalized,
            Err(_) => uri.clone(), // Fallback: return as-is
        },
        Err(_) => uri.clone(), // Not a valid file path — return as-is
    }
}
