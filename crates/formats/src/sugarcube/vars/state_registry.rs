//! State variable registry for SugarCube.

use std::collections::{HashMap, HashSet};

use knot_core::passage::VarKind;

use crate::types::{StateVariable, VarAccessKind, VarLocation};

/// Build a registry of all SugarCube state variables across the workspace.
///
/// This scans all passages for persistent variable references (`$var`,
/// `State.variables.var`, JS aliases) and collects them into a map from
/// dollar-prefixed name (e.g., "$hp") to `StateVariable`. Dot-notation
/// paths like `$player.name` are decomposed: the base variable (`$player`)
/// gets `name` added to its `known_properties`, and a separate base-level
/// read/write is also recorded.
///
/// Temporary variables (`_var`) are excluded from the registry since they
/// don't persist in `State.variables`.
pub(crate) fn build_state_variable_registry(
    workspace: &knot_core::Workspace,
) -> HashMap<String, StateVariable> {
    let mut registry: HashMap<String, StateVariable> = HashMap::new();

    for doc in workspace.documents() {
        let file_uri = doc.uri.to_string();
        for passage in &doc.passages {
            // Skip metadata passages
            if passage.is_metadata() {
                continue;
            }

            let passage_name = passage.name.clone();
            let is_special_seeding = passage.is_special
                && passage.special_def.as_ref().map_or(false, |d| d.contributes_variables);

            for var in &passage.vars {
                // Skip temporary variables — they don't persist in State.variables
                if var.is_temporary {
                    continue;
                }

                // Parse the variable name to extract base name and optional property path
                let (base_name, dollar_name, property_path) = parse_var_name(&var.name);

                let access_kind = match var.kind {
                    VarKind::Init => {
                        if let Some(path) = property_path.clone() {
                            VarAccessKind::PropertyWrite { path }
                        } else {
                            VarAccessKind::Assign
                        }
                    }
                    VarKind::Read => {
                        if let Some(path) = property_path.clone() {
                            VarAccessKind::PropertyRead { path }
                        } else {
                            VarAccessKind::Read
                        }
                    }
                };

                let location = VarLocation {
                    passage_name: passage_name.clone(),
                    file_uri: file_uri.clone(),
                    span: var.span.clone(),
                    kind: access_kind,
                };

                let entry = registry.entry(dollar_name.clone()).or_insert_with(|| {
                    StateVariable {
                        base_name: base_name.clone(),
                        dollar_name: dollar_name.clone(),
                        known_properties: HashSet::new(),
                        write_locations: Vec::new(),
                        read_locations: Vec::new(),
                        first_available: None,
                        seeded_by_special: false,
                    }
                });

                // Track known properties from dot-notation paths
                if let Some(ref path) = property_path {
                    entry.known_properties.insert(path.clone());
                }

                // Record the location in the appropriate list
                match &location.kind {
                    VarAccessKind::Assign | VarAccessKind::PropertyWrite { .. } => {
                        entry.write_locations.push(location);
                        // If this is in a special passage that contributes_variables,
                        // mark the variable as seeded by special
                        if is_special_seeding {
                            entry.seeded_by_special = true;
                        }
                    }
                    VarAccessKind::Read | VarAccessKind::PropertyRead { .. } => {
                        entry.read_locations.push(location);
                    }
                    VarAccessKind::Unset => {
                        // Unset doesn't go in either list, but we could track it
                        // separately in the future if needed
                    }
                }
            }
        }
    }

    registry
}

/// Parse a SugarCube variable name into its components.
///
/// - `"$hp"` → `("hp", "$hp", None)`
/// - `"$player.name"` → `("player", "$player", Some("name"))`
/// - `"$player.inventory.sword"` → `("player", "$player", Some("inventory.sword"))`
pub(crate) fn parse_var_name(name: &str) -> (String, String, Option<String>) {
    if let Some(dot_pos) = name.find('.') {
        let base = &name[..dot_pos];
        let path = &name[dot_pos + 1..];
        // base should start with $
        let base_name = if base.starts_with('$') {
            base[1..].to_string()
        } else {
            base.to_string()
        };
        let dollar_name = if base.starts_with('$') {
            base.to_string()
        } else {
            format!("${}", base)
        };
        (base_name, dollar_name, Some(path.to_string()))
    } else {
        let base_name = if name.starts_with('$') {
            name[1..].to_string()
        } else {
            name.to_string()
        };
        let dollar_name = if name.starts_with('$') {
            name.to_string()
        } else {
            format!("${}", name)
        };
        (base_name, dollar_name, None)
    }
}
