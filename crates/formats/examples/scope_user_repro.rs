// User's exact source. Reproduce the bleed they're seeing.
use knot_formats::sugarcube::SugarCubePlugin;
use knot_formats::plugin::FormatPluginMut;
use knot_formats::plugin::FormatPlugin as FormatPluginTrait;
use url::Url;

fn run(label: &str, src: &str, line: u32, char_: u32, trigger: Option<char>) {
    let uri = Url::parse("file:///project/story.tw").unwrap();
    let mut plugin = SugarCubePlugin::new();
    let result = plugin.parse_mut(&uri, &src);
    let mut workspace = knot_core::Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    let items = plugin.provide_completions(src, &workspace, &uri, line, char_, trigger, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    println!("--- {} ---", label);
    println!("  cursor: line={} char={} trigger={:?}", line, char_, trigger);
    println!("  labels: {:?}", labels);
    println!();
}

fn main() {
    println!("=== USER REPRO: temp var bleed ===\n");

    // User's exact source from Office passage
    let office = r#"::Office {"position":"1240,260"}

your manager looks at you questioning,
<<back>>
<<link "go back to locker" "locker">>
<</link>>
<<link "go to cafeteria" "Cafeteria">><</link>>
<<link "goto town" "newpassage">><</link>>
<<set _skd to value>>
"#;

    // StoryInit with another temp
    let story_init = ":: StoryInit\n<<set _initVar to 1>>\n";

    // A locker passage to test from
    let locker = "::Locker\nYou are at your locker.\n_\n";

    // Combined source
    let src = format!("{}{}{}", story_init, office, locker);

    println!("Full source:\n{}\n", src);

    // Print what was parsed
    let uri = Url::parse("file:///project/story.tw").unwrap();
    let mut plugin = SugarCubePlugin::new();
    let result = plugin.parse_mut(&uri, &src);
    println!("--- Parsed passages + vars ---");
    for passage in &result.passages {
        println!("Passage '{}':", passage.name);
        for var in &passage.vars {
            println!("  var: name={:?} kind={:?} is_temporary={} span={:?}",
                var.name, var.kind, var.is_temporary, var.span);
        }
    }
    println!();

    // Tree state
    let tree = plugin.registry().variables();
    println!("--- VariableTree state ---");
    println!("completion_names_for_passage(Some(Office)): {:?}",
        tree.completion_names_for_passage(Some("Office")));
    println!("completion_names_for_passage(Some(StoryInit)): {:?}",
        tree.completion_names_for_passage(Some("StoryInit")));
    println!("completion_names_for_passage(Some(Locker)): {:?}",
        tree.completion_names_for_passage(Some("Locker")));
    println!("completion_names_for_passage(None): {:?}",
        tree.completion_names_for_passage(None));
    println!();

    // Now test typing `_` in Locker passage (which has NO temps of its own)
    // Where is the `_` line in Locker? Let's count
    // StoryInit = 2 lines (0, 1)
    // Office = 11 lines (2..12): ::Office, blank, your manager, <<back>>, <<link, <</link>>, <<link, <<link, <<set _skd, blank
    // Locker = 3 lines: ::Locker, You are..., _
    // So `_` is at line 14
    println!("--- Testing `_` trigger in Locker (no temps of its own) ---");
    run("Locker `_` trigger — should be EMPTY (Locker has no temps)", &src, 14, 1, Some('_'));

    // Also test in Office
    println!("--- Testing `_` trigger in Office (has _skd) ---");
    // Find the line with _skd. Office starts at line 2.
    // ::Office = line 2
    // (blank) = line 3
    // your manager = line 4
    // <<back>> = line 5
    // <<link "go back..." "locker">> = line 6
    // <</link>> = line 7
    // <<link "go to cafeteria"... = line 8
    // <<link "goto town"... = line 9
    // <<set _skd to value>> = line 10
    // (blank) = line 11
    // ::Locker = line 12
    // You are at your locker. = line 13
    // _ = line 14
    // So to test `_` in Office, I need to add a `_` line in Office
    let src2 = format!("{}::Locker\nYou are at your locker.\n", story_init);
    let office_with_cursor = r#"::Office {"position":"1240,260"}

your manager looks at you questioning,
<<back>>
<<link "go back to locker" "locker">>
<</link>>
<<link "go to cafeteria" "Cafeteria">><</link>>
<<link "goto town" "newpassage">><</link>>
<<set _skd to value>>
_
"#;
    let src3 = format!("{}{}{}", story_init, office_with_cursor, "::Locker\nYou are at your locker.\n");
    println!("Full source with _ cursor in Office:\n{}\n", src3);
    // Now _ is at line 11
    run("Office `_` trigger — should be [_skd] only", &src3, 11, 1, Some('_'));

    // Test Ctrl+Space on _skd in Office
    // Line 10: <<set _skd to value>>
    //          0123456789012345
    // char 7 = '_', char 8 = 's', char 11 = 'd' (end of _skd), char 12 = ' '
    run("Office Ctrl+Space on _skd (char 8)", &src, 10, 8, None);
    run("Office Ctrl+Space at end of _skd (char 11)", &src, 10, 11, None);
}
