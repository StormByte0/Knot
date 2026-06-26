// Debug script: prints all semantic tokens emitted for sample expressions.
// Run with: cargo test -p knot-formats --lib debug_tokens_for_expression -- --nocapture
//
// Used during Phase 1 / A3 to verify per-token emission is working for
// <<=>>, <<run>>, <<print>>, and <<set>> expression bodies.

#![cfg(test)]
#[test]
fn debug_tokens_for_expression() {
    use crate::plugin::SemanticTokenType;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
    use crate::sugarcube::parser::parse_passage_body;
    use std::collections::HashSet;

    let samples: &[(&str, &str)] = &[
        ("<<=>>",       "<<= $hp + 5>>"),
        ("<<->>",       "<<- $hp + 5>>"),
        ("<<print>>",   "<<print $hp + 5>>"),
        ("<<run>>",     "<<run $hp + 5>>"),
        ("<<set>>",     "<<set $hp to 100>>"),
        ("<<set+str>>", "<<set $name to \"hello\">>"),
        ("<<set+prop>>","<<set $arr.last() to 5>>"),
        ("<<set+fn>>",  "<<set $x to random(1, 10)>>"),
        ("<<if>>",      "<<if $hp > 5 and $mp > 0>>body<</if>>"),
        ("verbatim",    "Before \"\"\"<<set $x to 1>> //italic//\"\"\" After"),
        ("def",         "<<if def $hp>>yes<</if>>"),
        ("ndef",        "<<if ndef $hp>>no<</if>>"),
        ("def+prop",    "<<if def $obj.prop>>yes<</if>>"),
        ("for-range",   "<<for $i from 1 to 10>>body<</for>>"),
        ("for-simple",  "<<for _i, $array>>body<</for>>"),
        ("for-cstyle",  "<<for _i to 0; _i lt 10; _i++>>body<</for>>"),
        ("img-link-sb", "[img[images/icon.png][Time]]"),
        ("img-link-sb-tip", "[img[Tooltip|images/icon.png][Time]]"),
        ("img-only-sb", "[img[images/icon.png]]"),
        ("italic",      "//italic text//"),
        ("bold",        "''bold text''"),
        ("underline",   "__underline text__"),
        ("strike",      "==strike text=="),
        ("sub",         "~~sub text~~"),
        ("super",       "^^super text^^"),
        ("markup-in-code", "<<code>>//italic// inside code<</code>>"),
        ("markup-in-if", "<<if true>>//italic// inside if<</if>>"),
        // E1: postfix/prefix ++/--
        ("set-post-inc", "<<set $a++>>"),
        ("set-pre-inc",  "<<set ++$a>>"),
        ("set-post-dec", "<<set $a-->>"),
        ("set-pre-dec",  "<<set --$a>>"),
        // E2: property functions
        ("run-last",     "<<run $arr.last()>>"),
        ("run-first",    "<<run $arr.first()>>"),
        ("run-includes", "<<run $arr.includes('x')>>"),
        ("run-count",    "<<run $arr.count()>>"),
        ("print-prop",   "<<= $arr.last()>>"),
        ("set-prop-lhs", "<<set $arr.last() to 5>>"),
        // E3: builtin functions in <<=>>
        ("print-random", "<<= random(1, 10)>>"),
        ("print-either", "<<= either('a', 'b')>>"),
        ("print-visited","<<= visited('Start')>>"),
        // H1: testbed audit samples
        ("tb-assign-eq", "<<set $a = 6>>"),
        ("tb-compound", "<<set $a += 1>>"),
        ("tb-js-eq", "<<if $a === 5>>yes<</if>>"),
        ("tb-js-neq", "<<if $a !== '5'>>yes<</if>>"),
        ("tb-logical-js", "<<if $b && !$c>>yes<</if>>"),
        ("tb-def", "<<if def $a>>yes<</if>>"),
        ("tb-ndef", "<<if ndef $missing>>no<</if>>"),
        ("tb-arith", "<<= 5 + 2>>"),
        ("tb-exp", "<<= 5 ** 2>>"),
        ("tb-member", "<<= $stats.strength>>"),
        ("tb-bracket", "<<= $inventory[0]>>"),
        ("tb-method-chain", "<<= $npcs.get(\"bard\").name>>"),
        ("tb-array-len", "<<= $inventory.length>>"),
        ("tb-ternary", "<<= $playerHP > 50 ? \"Healthy\" : \"Wounded\">>"),
        ("tb-string-method", "<<= \"hello world\".toUpperFirst()>>"),
        ("tb-clamp", "<<= (15).clamp(0, 10)>>"),
        ("tb-memorize", "<<run memorize(\"ach1\", true)>>"),
        ("tb-return", "<<return \"Return to start\">>"),
        // H1: <<code>> macro (removed from catalog in D1 — now parsed normally)
        ("tb-code-macro", "<<code>>$a to 5<</code>>"),
        ("tb-code-logical", "<<code>>$b && !$c<</code>>"),
        // H1: image link forms from testbed
        ("tb-img-link", "[img[images/icon.png][Time]]"),
        ("tb-img-link-pipe", "[img[Tooltip|images/icon.png][Time]]"),
        // H1: link macro single-arg (D2 — wrapper, not navigation)
        ("tb-link-single", "<<link \"Forest\">>body<</link>>"),
        ("tb-link-double", "<<link \"Talk\" \"Shop\">>body<</link>>"),
        // H1: verbatim block (A4) — content should NOT be highlighted
        ("tb-verbatim", "Before \"\"\"This //won't// be formatted as italics.\"\"\" After"),
        // H1: nowiki (similar to verbatim)
        ("tb-nowiki", "Before <nowiki>Same here — __no__ formatting.</nowiki> After"),
    ];

    for (label, src) in samples {
        println!("\n=== {} === ({:?})", label, src);
        let mut ast = parse_passage_body(src, 0, ParseMode::Normal);
        // is_script_passage=false (this is an inline expression, not a script passage)
        // sugarcube_syntax=true (SugarCube $var and keyword operators are allowed)
        crate::sugarcube::js::js_annotate::annotate_js(&mut ast, src, false, true, &HashSet::new());
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new(), "");
        // Sort by start position
        let mut sorted = tokens.clone();
        sorted.sort_by_key(|t| t.start);
        for t in &sorted {
            let snippet = &src[t.start..t.start + t.length];
            println!("  [{:3},{:3}) {:?} {:?}  {:?}",
                t.start, t.start + t.length, t.token_type, t.modifier, snippet);
        }
    }
}

#[test]
fn debug_js_analysis_for_expression() {
    // Bypass the token_builder and inspect JsAnalysis directly to see what
    // spans oxc + preprocessor actually produce.
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::js::js_annotate::annotate_js;
    use crate::sugarcube::parser::parse_passage_body;
    use std::collections::HashSet;

    let src = "<<= $hp + 5>>";
    println!("\n=== JsAnalysis for {:?} ===", src);
    let mut ast = parse_passage_body(src, 0, ParseMode::Normal);
    annotate_js(&mut ast, src, false, true, &HashSet::new());
    for node in &ast.nodes {
        if let crate::sugarcube::ast::AstNode::Expression { content, js_analysis, .. } = node {
            println!("content: {:?}", content);
            let analysis = js_analysis.as_ref().expect("js_analysis should be Some");
            println!("var_ops ({}):", analysis.var_ops.len());
            for op in &analysis.var_ops {
                println!("  {:?} {} span={:?}", op.access_kind, op.name, op.span);
            }
            println!("operator_spans ({}):", analysis.operator_spans.len());
            for op in &analysis.operator_spans {
                let snip = &src[op.span.start..op.span.end];
                println!("  {:?} span={:?} text={:?}", op.kind, op.span, snip);
            }
            println!("literal_spans ({}):", analysis.literal_spans.len());
            for lit in &analysis.literal_spans {
                let snip = &src[lit.span.start..lit.span.end];
                println!("  {:?} span={:?} text={:?}", lit.kind, lit.span, snip);
            }
            println!("function_calls ({}):", analysis.function_calls.len());
            for fc in &analysis.function_calls {
                println!("  {} span={:?}", fc.name, fc.span);
            }
            println!("js_method_spans ({}):", analysis.js_method_spans.len());
            for ms in &analysis.js_method_spans {
                let snip = if ms.start < src.len() && ms.end <= src.len() { &src[ms.start..ms.end] } else { "<OOB>" };
                println!("  span={:?} text={:?}", ms, snip);
            }
        }
    }
}

#[test]
fn debug_js_analysis_for_property_fn() {
    // Inspect JsAnalysis for <<run $arr.last()>> to understand E2 issue.
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::js::js_annotate::annotate_js;
    use crate::sugarcube::parser::parse_passage_body;
    use std::collections::HashSet;

    let src = "<<run $arr.last()>>";
    println!("\n=== JsAnalysis for {:?} ===", src);
    let mut ast = parse_passage_body(src, 0, ParseMode::Normal);
    annotate_js(&mut ast, src, false, true, &HashSet::new());
    for node in &ast.nodes {
        if let crate::sugarcube::ast::AstNode::Macro { name, args, js_analysis, .. } = node {
            println!("macro: {} args={:?}", name, args);
            let analysis = js_analysis.as_ref().expect("js_analysis should be Some");
            println!("var_ops ({}):", analysis.var_ops.len());
            for op in &analysis.var_ops {
                println!("  {:?} {} span={:?} segments={:?}",
                    op.access_kind, op.name, op.span, op.segment_spans);
            }
            println!("function_calls ({}):", analysis.function_calls.len());
            for fc in &analysis.function_calls {
                let snip = if fc.span.start < src.len() && fc.span.end <= src.len() { &src[fc.span.start..fc.span.end] } else { "<OOB>" };
                println!("  {} span={:?} text={:?}", fc.name, fc.span, snip);
            }
            println!("js_method_spans ({}):", analysis.js_method_spans.len());
            for ms in &analysis.js_method_spans {
                let snip = if ms.start < src.len() && ms.end <= src.len() { &src[ms.start..ms.end] } else { "<OOB>" };
                println!("  span={:?} text={:?}", ms, snip);
            }
            println!("js_property_spans ({}):", analysis.js_property_spans.len());
            for ps in &analysis.js_property_spans {
                let snip = if ps.start < src.len() && ps.end <= src.len() { &src[ps.start..ps.end] } else { "<OOB>" };
                println!("  span={:?} text={:?}", ps, snip);
            }
        }
    }
}
