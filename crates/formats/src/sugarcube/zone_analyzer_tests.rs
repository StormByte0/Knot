//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).
//! Phase 4.1 ZoneAnalyzer (plan.md): the fixture matrix pins per-byte//! `language_at` answers across every stratum, and the zone invariants//! (sorted, non-empty, non-overlapping leaves) are validated over a corpus//! of representative/hostile fixtures.
use super::*;
use knot_core::zoning::Language;

#[test]
fn language_at_agrees_with_expected_strata() {
    // Fixture matrix: every stratum in one passage body.
    let body = "Prose [[Forest]] <<set $x to 1>> <div>in</div>\n<script>var a;</script>\n<style>.b{}</style>\n";
    let plugin = SugarCubePlugin::new();
    let zones = plugin.zone_analyze(body);
    zones.validate().expect("zone invariants hold");

    let at = |needle: &str, skip: usize| body.find(needle).unwrap() + skip;
    // Prose and wikified constructs are Markup.
    assert_eq!(zones.language_at(at("Prose", 0)), Language::Markup);
    assert_eq!(zones.language_at(at("[[Forest]]", 3)), Language::Markup);
    assert_eq!(zones.language_at(at("<<set", 1)), Language::Markup);
    // Tag interiors are Html; element content is Markup (wikified).
    assert_eq!(zones.language_at(at("<div", 2)), Language::Html);
    assert_eq!(zones.language_at(at("<div>", 5)), Language::Markup);
    // Raw-text bodies are Js / Css.
    assert_eq!(zones.language_at(at("<script>", 8)), Language::Js);
    assert_eq!(zones.language_at(at("<style>", 7)), Language::Css);
    // Uncovered gaps default to Markup.
    assert_eq!(zones.language_at(body.len()), Language::Markup);
}

#[test]
fn zone_invariants_hold_on_corpus() {
    // Whole-corpus invariant check: representative fixtures from every
    // phase's strata plus hostile shapes (broken JS, literal `<`,
    // symbols inside attributes).
    let corpus = [
        ":: A\nplain prose\n",
        ":: A\n''bold [[link]]'' and <<set $x to 1>>\n",
        ":: A\n<div class=\"hud\" @style=\"$x\">''b''</div>\n<<if $x>>y<</if>>\n",
        ":: A\n<script>\nvar hp = ;\nvar ok = 1;\n</script>\n",
        ":: A\n<style>.hud { color: red; }</style>\n",
        ":: A\n<textarea>''x''</textarea>\n5 < 6 and <3\n",
        ":: A\n<<script>>\nbad {\n<</script>>\n[[L]]\n",
        ":: A\n<span title=\"a <<if>> [[x]]\">t</span> [[L]]\n",
    ];
    let plugin = SugarCubePlugin::new();
    for body in corpus {
        let label = body.split('\n').next().unwrap_or("").to_string();
        let zones = plugin.zone_analyze(body);
        if let Err(e) = zones.validate() {
            panic!("zone invariants violated for fixture {label:?}: {e}");
        }
    }
}
