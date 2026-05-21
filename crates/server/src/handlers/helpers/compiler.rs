//! Compiler detection, version detection, and graph metric computation.

use knot_core::Workspace;

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

/// Compute the maximum depth from the start passage using BFS.
pub(crate) fn compute_max_depth(workspace: &Workspace, start_passage: &str) -> u32 {
    let mut depths: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    if workspace.graph.contains_passage(start_passage) {
        depths.insert(start_passage.to_string(), 0);
        queue.push_back(start_passage.to_string());
    }

    while let Some(name) = queue.pop_front() {
        let current_depth = *depths.get(&name).unwrap_or(&0);
        for neighbor in workspace.graph.outgoing_neighbors(&name) {
            if !depths.contains_key(&neighbor) {
                let new_depth = current_depth + 1;
                depths.insert(neighbor.clone(), new_depth);
                queue.push_back(neighbor);
            }
        }
    }

    depths.values().copied().max().unwrap_or(0)
}

/// Compute the number of weakly connected components in the passage graph.
pub(crate) fn compute_connected_components(workspace: &Workspace) -> u32 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    if passage_names.is_empty() {
        return 0;
    }

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut component_count: u32 = 0;

    for name in &passage_names {
        if visited.contains(name) {
            continue;
        }

        // BFS considering both directions (weakly connected)
        component_count += 1;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(name.clone());
        visited.insert(name.clone());

        while let Some(current) = queue.pop_front() {
            for neighbor in workspace.graph.outgoing_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
            for neighbor in workspace.graph.incoming_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    component_count
}

/// Compute a simplified average clustering coefficient.
///
/// For each passage, count how many of its outgoing neighbors also link
/// to each other (forming triangles), divided by the maximum possible
/// number of such connections. Returns the average across all passages
/// with at least 2 outgoing links.
pub(crate) fn compute_avg_clustering(workspace: &Workspace) -> f64 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    let mut coefficients: Vec<f64> = Vec::new();

    for name in &passage_names {
        let out_neighbors: Vec<String> = workspace.graph.outgoing_neighbors(name);
        let k = out_neighbors.len();

        if k < 2 {
            continue;
        }

        let neighbor_set: std::collections::HashSet<String> =
            out_neighbors.iter().cloned().collect();

        let mut triangle_count: u32 = 0;
        for neighbor in &out_neighbors {
            let their_neighbors = workspace.graph.outgoing_neighbors(neighbor);
            for their_target in &their_neighbors {
                if neighbor_set.contains(their_target) {
                    triangle_count += 1;
                }
            }
        }

        let max_possible = (k * (k - 1)) as f64;
        let local_coeff = triangle_count as f64 / max_possible;
        coefficients.push(local_coeff);
    }

    if coefficients.is_empty() {
        0.0
    } else {
        coefficients.iter().sum::<f64>() / coefficients.len() as f64
    }
}
