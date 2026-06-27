//! Built-in SugarCube global object definitions and hover text.
//!
//! Self-contained module providing the catalog of SugarCube global objects
//! (State, Engine, Story, etc.) and hover documentation for them.

use crate::types::{GlobalDef, GlobalProperty};

/// Built-in SugarCube global object definitions.
pub fn builtin_globals() -> &'static [GlobalDef] {
    use GlobalProperty as GP;
    static GLOBALS: &[GlobalDef] = &[
        GlobalDef {
            name: "State",
            description: "SugarCube state management API.",
            properties: Some(&[
                GP { name: "variables",    description: "Record<string, unknown> — story variables", is_method: false },
                GP { name: "temporary",    description: "Record<string, unknown> — temporary variables", is_method: false },
                GP { name: "turns",        description: "number — turn count", is_method: false },
                GP { name: "passage",      description: "string — current passage name", is_method: false },
                GP { name: "active",       description: "object — active passage info", is_method: false },
                GP { name: "top",          description: "object — top passage info", is_method: false },
                GP { name: "history",      description: "array — passage history", is_method: false },
                GP { name: "has()",        description: "boolean — check if passage visited", is_method: true },
                GP { name: "hasTag()",     description: "boolean — check if tag visited", is_method: true },
                GP { name: "index",        description: "number — current history index", is_method: false },
                GP { name: "size",         description: "number — history size", is_method: false },
            ]),
        },
        GlobalDef {
            name: "Engine",
            description: "Story engine control API.",
            properties: Some(&[
                GP { name: "play()",       description: "void — navigate to passage", is_method: true },
                GP { name: "forward()",    description: "void — go forward in history", is_method: true },
                GP { name: "backward()",   description: "void — go backward in history", is_method: true },
                GP { name: "goto()",       description: "void — navigate to passage", is_method: true },
                GP { name: "isIdle()",     description: "boolean — is engine idle", is_method: true },
                GP { name: "isPlaying()",  description: "boolean — is engine playing", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Story",
            description: "Story metadata and passage lookup API.",
            properties: Some(&[
                GP { name: "title",   description: "string — story title", is_method: false },
                GP { name: "has()",   description: "boolean — check passage exists", is_method: true },
                GP { name: "get()",   description: "object — get passage data", is_method: true },
                GP { name: "filter()", description: "array — filter passages", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Save",
            description: "Save/load API.",
            properties: Some(&[
                GP { name: "save()",    description: "void — save game", is_method: true },
                GP { name: "load()",    description: "void — load game", is_method: true },
                GP { name: "delete()",  description: "void — delete save", is_method: true },
                GP { name: "ok()",      description: "boolean — check save exists", is_method: true },
                GP { name: "sizes()",   description: "object — save sizes", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Config",
            description: "Story configuration object.",
            properties: Some(&[
                GP { name: "debug",       description: "boolean — debug mode", is_method: false },
                GP { name: "history",     description: "object — history config", is_method: false },
                GP { name: "macros",      description: "object — macro config", is_method: false },
                GP { name: "navigation",  description: "object — navigation config", is_method: false },
                GP { name: "ui",          description: "object — UI config", is_method: false },
            ]),
        },
        GlobalDef {
            name: "UI",
            description: "UI utility API.",
            properties: Some(&[
                GP { name: "alert()",    description: "void — show alert dialog", is_method: true },
                GP { name: "restart()",  description: "void — restart story", is_method: true },
                GP { name: "squash()",   description: "void — squash history", is_method: true },
                GP { name: "goto()",     description: "void — navigate to passage", is_method: true },
                GP { name: "include()",  description: "void — include passage", is_method: true },
            ]),
        },
        GlobalDef { name: "Dialog",      description: "Dialog box API.", properties: None },
        GlobalDef { name: "Fullscreen",  description: "Fullscreen API.", properties: None },
        GlobalDef { name: "LoadScreen",  description: "Loading screen API.", properties: None },
        GlobalDef { name: "Macro",       description: "Macro registration API (e.g. Macro.add).", properties: None },
        GlobalDef { name: "Passage",     description: "Current passage info.", properties: None },
        GlobalDef { name: "Setting",     description: "Settings API.", properties: None },
        GlobalDef { name: "Settings",    description: "Settings object.", properties: None },
        GlobalDef { name: "SimpleAudio", description: "Simple audio API.", properties: None },
        GlobalDef { name: "Template",    description: "Template API.", properties: None },
        GlobalDef { name: "UIBar",       description: "Story navigation bar API.", properties: None },
        GlobalDef { name: "SugarCube",   description: "Global SugarCube namespace.", properties: None },
        GlobalDef { name: "setup",       description: "Author setup object for shared data.", properties: None },
        GlobalDef { name: "prehistory",  description: "Prehistory task array.", properties: None },
        GlobalDef { name: "predisplay",  description: "Predisplay task array.", properties: None },
        GlobalDef { name: "prerender",   description: "Prerender task array.", properties: None },
        GlobalDef { name: "postdisplay", description: "Postdisplay task array.", properties: None },
        GlobalDef { name: "postrender",  description: "Postrender task array.", properties: None },
    ];
    GLOBALS
}

/// Hover text for SugarCube global objects.
///
/// Returns rich Markdown hover text for known SugarCube globals like
/// `State`, `Engine`, `Story`, `Save`, `Config`, `UI`, etc.
pub fn global_hover_text(name: &str) -> Option<&'static str> {
    match name {
        "State"      => Some("**SugarCube** `State` — the story history and variable store."),
        "Engine"     => Some("**SugarCube** `Engine` — controls passage navigation."),
        "Story"      => Some("**SugarCube** `Story` — passage access and metadata."),
        "SugarCube"  => Some("**SugarCube** version metadata object."),
        "setup"      => Some("**SugarCube** `setup` — author-defined initialisation object."),
        "passage"    => Some("**SugarCube** `passage` — title of the current passage."),
        "tags"       => Some("**SugarCube** `tags` — tag array of the current passage."),
        "visited"    => Some("**SugarCube** `visited(...passages)` — times any listed passage was visited."),
        "turns"      => Some("**SugarCube** `turns` — number of turns elapsed."),
        "time"       => Some("**SugarCube** `time` — milliseconds since last `<<timed>>` or `<<repeat>>`."),
        "$args"      => Some("**SugarCube** `$args` — arguments passed to the current `<<widget>>`."),
        "Dialog"     => Some("**SugarCube** `Dialog` — dialog box API."),
        "Fullscreen" => Some("**SugarCube** `Fullscreen` — fullscreen API."),
        "LoadScreen" => Some("**SugarCube** `LoadScreen` — loading screen API."),
        "Macro"      => Some("**SugarCube** `Macro` — macro registration API (e.g. Macro.add)."),
        "Passage"    => Some("**SugarCube** `Passage` — current passage info."),
        "Save"       => Some("**SugarCube** `Save` — save/load API."),
        "Setting"    => Some("**SugarCube** `Setting` — settings API."),
        "Settings"   => Some("**SugarCube** `Settings` — settings object."),
        "SimpleAudio"=> Some("**SugarCube** `SimpleAudio` — simple audio API."),
        "Template"   => Some("**SugarCube** `Template` — template API."),
        "UI"         => Some("**SugarCube** `UI` — UI utility API."),
        "UIBar"      => Some("**SugarCube** `UIBar` — story navigation bar API."),
        "Config"     => Some("**SugarCube** `Config` — story configuration object."),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Builtin standalone functions
// ---------------------------------------------------------------------------

/// SugarCube builtin standalone functions callable in TwineScript without
/// any object prefix (e.g., `random(1, 10)`, `either("a", "b")`).
///
/// These are NOT the same as global objects like `State` or `Engine` —
/// those are namespaces accessed via dot notation. Builtin functions
/// are called directly by name.
///
/// Source: SugarCube 2 docs — "Functions" section.
pub fn builtin_functions() -> &'static [(&'static str, &'static str)] {
    static FUNCTIONS: &[(&str, &str)] = &[
        // ── Value functions ───────────────────────────────────────────
        ("clone",         "Deep-clones a value. Useful for copying objects/arrays before mutation."),
        ("either",        "Returns one of the given arguments at random. Often used with `<<set>>` and `<<link>>`."),

        // ── Random functions ──────────────────────────────────────────
        ("random",        "Returns a random integer in the inclusive range [min, max]."),
        ("randomFloat",   "Returns a random floating-point number in the inclusive range [min, max]."),

        // ── History / passage functions ───────────────────────────────
        ("visited",       "Returns the number of times the given passage(s) have been visited. With no args, returns the current passage's visit count."),
        ("visitedTags",   "Returns the number of times passages with ALL the given tag(s) have been visited."),
        ("hasVisited",    "Returns whether the given passage(s) have been visited at least once."),
        ("lastVisited",   "Returns the Unix timestamp of the last time the given passage was visited. With no args, uses the current passage."),
        ("passage",       "Returns the name of the current passage. Optionally accepts a passage name to check."),
        ("previous",      "Returns the name of the previous passage (the passage shown before the current one)."),
        ("tags",          "Returns the tags of the current passage as an array. Optionally accepts a passage name."),

        // ── Session functions ─────────────────────────────────────────
        ("turns",         "Returns the total number of turns played in the current session."),
        ("time",          "Returns the number of milliseconds elapsed since the last `<<timed>>` or `<<repeat>>` macro was triggered."),

        // ── Memorize / recall functions ───────────────────────────────
        ("memorize",      "Stores a value under the given name. Persisted across sessions via localStorage."),
        ("recall",        "Retrieves a previously memorized value. Returns `undefined` if not found."),
        ("forget",        "Removes a previously memorized value."),

        // ── DOM / page functions ──────────────────────────────────────
        ("setPageElement","Sets the content of a DOM element by ID from a passage. Useful for AJAX-like updates."),
        ("triggerEvent",  "Triggers a custom event on the document. Useful for hooking into SugarCube's event system."),
        ("importScripts", "Dynamically imports one or more external JavaScript files. Returns a Promise."),
        ("importStyles",  "Dynamically imports one or more external CSS files. Returns a Promise."),
    ];

    FUNCTIONS
}

/// Check if a name is a SugarCube builtin function.
pub fn is_builtin_function(name: &str) -> bool {
    builtin_functions().iter().any(|(n, _)| *n == name)
}

/// Get the description for a SugarCube builtin function.
pub fn describe_builtin_function(name: &str) -> Option<&'static str> {
    builtin_functions()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, desc)| *desc)
}

// ---------------------------------------------------------------------------
// SugarCube extension methods (Array, String, Number prototypes)
// ---------------------------------------------------------------------------

/// SugarCube extension methods added to JavaScript prototypes at runtime.
///
/// SugarCube extends `Array.prototype`, `String.prototype`, and
/// `Number.prototype` with additional methods. These are NOT standard JS —
/// they're injected by the SugarCube runtime. oxc parses them as valid
/// member-expression calls (syntactically correct), but the LSP has no
/// way to know they're valid SugarCube methods without this catalog.
///
/// Source: SugarCube 2 docs — "Native Object Methods" section.
pub fn builtin_methods() -> &'static [(&'static str, &'static str)] {
    static METHODS: &[(&str, &str)] = &[
        // ── Array methods (SugarCube extensions) ──────────────────────
        ("first",        "Array: Returns the first element of the array. Returns `undefined` if the array is empty."),
        ("last",         "Array: Returns the last element of the array. Returns `undefined` if the array is empty."),
        ("includes",     "Array/String: Returns whether the given value exists within the array/string. (SugarCube's version, not the native ES2016 method.)"),
        ("includesAll",  "Array: Returns whether ALL of the given values exist within the array."),
        ("includesAny",  "Array: Returns whether ANY of the given values exist within the array."),
        ("pushUnique",   "Array: Appends one or more elements to the end of the array, but only if they're not already present. Returns the new length."),
        ("deleteWith",   "Array: Removes all elements that match the given predicate function. Returns the array for chaining."),
        ("deleteAt",     "Array: Removes the element at the given index(es). Returns the array for chaining."),
        ("count",        "Array/String: Returns the number of elements/characters that match the given value or predicate."),
        ("toShuffled",   "Array: Returns a new array with the same elements in random order. Does not modify the original."),
        ("random",       "Array: Returns a random element from the array. Also works on the `random()` builtin function."),
        ("flatten",      "Array: Returns a new array with all sub-array elements concatenated into it recursively up to the specified depth."),
        ("toUpperFirst", "String: Returns a copy of the string with the first character converted to uppercase."),
        ("clamp",        "Number: Returns the value clamped to the inclusive range [min, max]. Example: `(15).clamp(0, 10)` returns `10`."),
        // Note: .length is a native JS property, not a SugarCube extension,
        // but we include it here so hover works on $arr.length.
        ("length",       "Array/String: Returns the number of elements/characters. (Native JS property, not a SugarCube extension.)"),
    ];

    METHODS
}

/// Check if a name is a SugarCube extension method.
pub fn is_builtin_method(name: &str) -> bool {
    builtin_methods().iter().any(|(n, _)| *n == name)
}

/// Get the description for a SugarCube extension method.
pub fn describe_builtin_method(name: &str) -> Option<&'static str> {
    builtin_methods()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, desc)| *desc)
}
