//! Compiler detection and version detection.

/// Search for the Tweego compiler on the system PATH.
///
/// On Unix systems, uses `which` to locate the binary.
/// On Windows, uses `where` instead (the `which` command does not exist).
/// Falls back to trying direct execution with `--version` if the
/// system locator is unavailable.
pub(crate) fn which_compiler() -> Option<std::path::PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["tweego.exe"]
    } else {
        &["tweego"]
    };

    // Use the platform-appropriate locator command
    let locator = if cfg!(windows) { "where" } else { "which" };

    for name in candidates {
        if let Ok(output) = std::process::Command::new(locator)
            .arg(name)
            .output()
            && output.status.success() {
                // `where` on Windows may return multiple lines; take the first.
                let path_str = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    return Some(path);
                }
            }
    }

    // Fallback: try direct execution — if the binary is on PATH,
    // running it with --version will succeed.
    for name in candidates {
        if std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(std::path::PathBuf::from(name));
        }
    }

    None
}

/// Detect the version string of a compiler by running `--version`.
pub(crate) async fn detect_compiler_version(path: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout);
        // Take the first line of output as the version string
        Some(version.lines().next().unwrap_or("").to_string())
    } else {
        None
    }
}
