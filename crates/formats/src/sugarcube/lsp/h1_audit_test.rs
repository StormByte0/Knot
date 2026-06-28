// H1 audit: parse every testbed .twee file and report any panics,
// error nodes, or unexpected token patterns.
//
// Run with: cargo test -p knot-formats --lib h1_testbed_audit -- --nocapture

#![cfg(test)]
#[test]
fn h1_testbed_audit() {
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;
    use std::collections::HashSet;
    use std::path::Path;

    let testbed_dir = Path::new("/home/z/my-project/sugarcube-testbed/src");
    if !testbed_dir.exists() {
        eprintln!("testbed directory not found: {:?}", testbed_dir);
        return;
    }

    let mut files_checked = 0;
    let mut passages_checked = 0;
    let mut total_error_nodes = 0;
    let mut total_tokens = 0;
    let mut issues: Vec<String> = Vec::new();

    // Read all .twee files sorted
    let mut entries: Vec<_> = std::fs::read_dir(testbed_dir)
        .expect("read testbed dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "twee"))
        .collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                issues.push(format!("{}: READ ERROR: {}", filename, e));
                continue;
            }
        };
        files_checked += 1;

        // Split into passages using the lexer
        let raw_passages = crate::sugarcube::lexer::split_passages(&content);
        for (header, body) in &raw_passages {
            passages_checked += 1;
            let passage_name = &header.name;

            // Parse the body
            let ast = parse_passage_body(body, 0, ParseMode::Normal);

            // Count error nodes
            fn count_errors(nodes: &[crate::sugarcube::ast::AstNode]) -> usize {
                let mut n = 0;
                for node in nodes {
                    match node {
                        crate::sugarcube::ast::AstNode::Error { .. } => n += 1,
                        crate::sugarcube::ast::AstNode::Macro { children, .. } => {
                            n += count_errors(children.as_deref().unwrap_or(&[]));
                        }
                        crate::sugarcube::ast::AstNode::InlineStyle { children, .. } => {
                            n += count_errors(children);
                        }
                        _ => {}
                    }
                }
                n
            }
            let error_count = count_errors(&ast.nodes);
            total_error_nodes += error_count;
            if error_count > 0 {
                issues.push(format!(
                    "{} :: {}: {} error node(s)",
                    filename, passage_name, error_count
                ));
            }

            // Build tokens to check for crashes
            let mut tokens = Vec::new();
            crate::sugarcube::lsp::token_builder::build_semantic_tokens(
                &ast.nodes,
                &mut tokens,
                0,
                &HashSet::new(),
                body,
            );
            total_tokens += tokens.len();
        }
    }

    println!("=== H1 Testbed Audit ===");
    println!("Files checked: {}", files_checked);
    println!("Passages checked: {}", passages_checked);
    println!("Total error nodes: {}", total_error_nodes);
    println!("Total tokens emitted: {}", total_tokens);
    println!("\nIssues found ({}):", issues.len());
    for issue in &issues {
        println!("  - {}", issue);
    }
}
