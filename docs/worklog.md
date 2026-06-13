# Knot Worklog

---
Task ID: 1
Agent: Main
Task: Update plan.md with template research findings

Work Log:
- Read user's template research findings: Template.add uses similar API to Macro.add, can create registries just like custom macros
- Updated plan.md "Template API" section from "Deferred" with open questions to "Research Complete" with concrete findings
- Added 5 research findings: registration API, invocation syntax (?templateName), global scope, runtime validation, unknown chaining
- Added implementation plan section with what's missing for template support

Stage Summary:
- plan.md updated with template research answers
- Template registration already works via existing JsAnalysis.template_adds and TemplateRegistry
- Template invocation (?name) still needs parser detection and AST node support

---
Task ID: 2
Agent: Main
Task: Phase 1 validation — confirm literal/operator token implementation is complete

Work Log:
- Verified all helper functions in js_walk.rs are fully implemented (emit_assignment_operator, emit_binary_operator, emit_logical_operator, emit_unary_operator, emit_update_operator, extract_substitution_operators)
- Verified token_builder.rs has emit_literal_tokens and emit_operator_tokens functions
- cargo check passes, cargo test passes (519 tests, 0 failures)

Stage Summary:
- Phase 1 was already complete from previous session
- All literal/operator span types, population in js_walk, and token emission in token_builder are working

---
Task ID: 3
Agent: Main
Task: Phase 2 — Namespace tokens for global objects

Work Log:
- Added NamespaceSpan and PropertySpan types to ast.rs
- Added namespace_spans field to JsAnalysis struct
- Added is_known_global() helper using builtin_globals() catalog
- Added emit_namespace_for_member_expr() in js_walk.rs — emits NamespaceSpan when StaticMemberExpression's object is a known global
- Added skip conditions to avoid token overlap:
  - State.variables and State.temporary → already handled by var_ops
  - Macro.add and Template.add → already handled by call expression handler
  - SugarCube.State → always prefix of variable chain handled by var_ops
  - SugarCube.Macro and SugarCube.Template → prefix for add patterns
- Decided NOT to emit standalone NamespaceSpan from Identifier handler to avoid double-emission when recursing into member expression objects
- Added emit_namespace_tokens() function in token_builder.rs — emits Namespace + Property tokens
- Wired namespace token emission into build_semantic_tokens for both Macro and Expression AST nodes
- cargo check passes, cargo test passes (519 tests, 0 failures)

Stage Summary:
- Phase 2 complete: SugarCube global objects (Engine, Story, Config, Save, UI, etc.) now get Namespace + Property semantic tokens
- Key design decisions:
  - Only emit namespace at the StaticMemberExpression level where object is a direct global identifier
  - SugarCube.Something chains handled by recursion to inner member expression
  - Deduplication with var_ops achieved through explicit skip conditions rather than post-hoc dedup
  - No standalone namespace emission for Identifier nodes (avoids double-emission from recursion)

---
Task ID: 4
Agent: Main
Task: Phase 3 — Function & Widget Token Differentiation

Work Log:
- Added `definition_name_span: Option<Range<usize>>` field to `AstNode::Macro` in ast.rs
  - Captures the span of the name being defined in `<<widget myHelper>>`
- Added `parse_definition_name_span()` function in macro_parser.rs
  - Scans first identifier from args after optional whitespace
  - Only called for `<<widget>>` macros (name.eq_ignore_ascii_case("widget"))
- Updated all 3 `AstNode::Macro` construction sites in macro_parser.rs to include `definition_name_span`
- Changed `build_semantic_tokens()` signature to accept `custom_macro_names: &HashSet<String>`
  - Macro names in `custom_macro_names` get `Function` token type; others get `Macro`
  - `<<widget myHelper>>`: `Macro` token on "widget" keyword, `Function + Definition` on "myHelper" span
- Added `emit_function_def_tokens()` in token_builder.rs
  - Emits `Function + Definition` tokens for `JsAnalysis.function_defs` entries
  - Covers `function myHelper() {}`, `var calc = function() {}`, `const add = () => {}`
- Updated parse_pipeline.rs to collect custom macro names from registry and pass to token_builder
  - Fixed borrow checker issue: used `registry.custom_macros()` instead of `plugin.registry()`
- cargo check passes, cargo test passes (519 tests, 0 failures)

Stage Summary:
- Phase 3 complete: Built-in macros get `Macro` tokens, custom macros get `Function` tokens
- Widget definitions emit `Function + Definition` for the defined name
- JS function definitions inside macros/expressions emit `Function + Definition` tokens
- Combined with Phase 7 from the original plan (function token emission from JsAnalysis.function_defs)

---
Task ID: 5
Agent: Main
Task: Comprehensive audit — verify plan.md decisions were followed, identify gaps, catch up worklog

Work Log:
- Re-read full plan.md (all 8 phases + dependency graph + risk assessment)
- Read ast.rs: verified JsAnalysis fields match plan (literal_spans, operator_spans, namespace_spans, function_defs)
- Read token_builder.rs: verified all emission functions present (emit_literal_tokens, emit_operator_tokens, emit_namespace_tokens, emit_function_def_tokens)
- Read js_walk.rs: verified namespace emission with skip conditions for var_ops overlap
- Read macro_parser.rs: verified definition_name_span for <<widget>>, verified block/inline logic
- Read predicates.rs: found predicates::is_block_macro() adds names on top of classifiers::block_macro_names()
- Read classifiers.rs: found separate hardcoded block_macro_names() set
- Read lookup.rs: found THIRD is_block_macro() with yet another hardcoded list, plus deprecated_macros()
- Read catalog.rs: verified MacroDef has deprecated + deprecation_message fields, but no polymorphy field
- Read parse_pipeline.rs: verified custom_macro_names passed to token builder
- Ran cargo check — compiles clean

Audit Findings:

## Phase 1 (Literal & Operator Tokens) — COMPLETE ✅
- Matches plan.md Section 1.2 exactly: LiteralSpan, LiteralKind, OperatorSpan, OperatorKind all present
- Matches Section 1.4: emit_literal_tokens and emit_operator_tokens in token_builder.rs
- Null → Keyword mapping matches plan
- Logical operators → ControlFlow modifier matches plan
- Section 1.5 (fallback path): correctly not emitting literal/operator tokens when js_analysis is None

## Phase 2 (Namespace Tokens) — COMPLETE ✅ (with deviation)
- Matches plan Section 2.1: NamespaceSpan and PropertySpan present in ast.rs
- Matches Section 2.3: emit_namespace_tokens in token_builder.rs
- **Deviation from plan Section 2.4**: Plan's "Preferred approach" was to change AnalyzedVarOp emission
  so State.variables root segment emits Namespace instead of Variable. Implementation chose the
  "skip condition" approach instead — namespace emission skips State.variables/temporary entirely,
  leaving var_ops to emit Variable+Property. This is functionally equivalent but the plan's preferred
  approach would be semantically cleaner (State IS a namespace, not a variable). Not a bug, but a
  design decision worth revisiting.

## Phase 3 (Function & Widget Token Differentiation) — COMPLETE ✅
- Matches plan Section 3.1 Option A: custom_macro_names passed as parameter to build_semantic_tokens
- Matches Section 3.2 Option 1: definition_name_span field added to AstNode::Macro
- Phase 7 (Function Extraction Token Emission) was folded in: emit_function_def_tokens handles
  JsAnalysis.function_defs — this is mentioned in the worklog as "combined with Phase 7"

## Phase 4 (Deprecated Macro Modifier) — NOT STARTED ❌
- No Deprecated modifier on macro name tokens
- No deprecation diagnostics emitted
- deprecated_macros() exists in lookup.rs and catalog has deprecated fields — just not wired to tokens

## Phase 5 (Contextual Macro Parsing) — NOT STARTED ❌
- No capture_target, for_loop_vars, or generalized definition_name fields
- Only set_assignment and definition_name_span exist

## Phase 6 (Structured Args from Catalog) — NOT STARTED ❌
- No structured_args on AstNode::Macro
- No passage ref extraction from args in parser

## Phase 7 (Function Token Emission) — COMPLETE ✅ (merged into Phase 3)
- emit_function_def_tokens handles JsAnalysis.function_defs
- Function + Definition tokens for function declarations/expressions

## Phase 8 (Cleanup) — NOT STARTED ❌
- INLINE_JS_MACROS still hardcoded in js_annotate.rs
- Three divergent block-macro lists (see below)

## Critical Gap: Macro Polymorphy 🔴
- SugarCube macros like <<link>>, <<button>>, <<linkappend>>, <<linkprepend>>, <<linkreplace>>
  can be used in TWO forms:
  1. Inline: `<<link "label" "passage">>` (no body, no close tag)
  2. Block: `<<link "label">>...<</link>>` (has body, has close tag)
- The parser ALWAYS treats these as block macros because `is_block_macro("link")` returns true
- This means `<<link "Go" "Room">>` (inline form, no close tag) causes the parser to consume
  THE ENTIRE REST OF THE PASSAGE as the macro's children, which is a parse bug
- MacroDef has no `polymorphic` or `can_be_inline` field — `has_body: true` is a single boolean
- No arg-based reclassification exists in the parser

## Critical Gap: Block-Macro List Drift 🔴
Three separate lists with different contents:
1. classifiers::block_macro_names() — 24 names (base set, includes elseif/else/case/default)
2. lookup::is_block_macro() — 26 names (adds timed, repeat, createplaylist, css)
3. predicates::is_block_macro() — ~28 names (classifiers + widget, script, style, css, nobr, silently, done, capture)

Missing from parser's list (predicates): timed, repeat, createplaylist
This means `<<timed>>...<</timed>>` is NOT parsed as a block macro — it's treated as inline!

## Moderate Gap: Deprecated Macros Duplication 🟡
- deprecated_macros() in lookup.rs manually duplicates the deprecated/deprecation_message
  fields from catalog.rs. If one is updated without the other, they'll disagree.

## Minor Gap: No Block/Inline Discriminator on AstNode::Macro 🟡
- Downstream consumers must infer inline-vs-block from children/close_span
- An explicit `is_block: bool` field would make this clearer and support polymorphy

Stage Summary:
- Phase 1, 2, 3, 7 are complete and match plan.md decisions (Phase 2 has a minor deviation)
- Phase 4, 5, 6, 8 are not started
- Two critical gaps discovered: macro polymorphy (parse bug for inline link/button) and
  block-macro list drift (timed/repeat/createplaylist missing from parser's list)
- One moderate gap: deprecated macro duplication between lookup.rs and catalog.rs
- Phase 3+7 merge was a good decision — the plan noted they could run in parallel

---
Task ID: 6
Agent: Main
Task: Parser refactor — flat parse + tree build (fixes polymorphy bug and block-list drift)

Work Log:
- Added BodyRequirement enum to types.rs (Never/Optional/Required)
- Replaced has_body: bool with body: BodyRequirement in MacroDef
- Updated all 58 macro catalog entries with correct BodyRequirement values
- link, button, click → Optional (polymorphic)
- if, for, switch, widget, capture, etc. → Required (always block)
- set, print, goto, run, etc. → Never (always inline)
- Updated build_macro_snippet() trait method and all format plugin implementations
- Added MacroClose variant to AstNode enum
- Added close_name_span field to AstNode::Macro (lossless AST)
- Refactored macro_parser.rs for flat emission:
  - Close tags emit MacroClose instead of Macro
  - Removed is_block_macro() and is_block_modifier() calls
  - Removed parse_block_body() recursive call
  - All macros emitted with children: None, close_span: None
  - Script/style/css still pre-nested (raw body extraction)
  - Added parse_raw_body() for script/style opaque body capture
- Created tree_builder.rs with stack-based pairing algorithm:
  - Pairs Macro with MacroClose, establishes nesting
  - Consults catalog's BodyRequirement for unmatched open macros
  - Optional with no close → inline (no error)
  - Required with no close → unclosed block (children = remaining, diagnostic)
  - Never → inline regardless
  - Orphan close tags → Error node
- Wired tree builder into parse_pipeline (Phase 1.5)
- Cleaned up dead code:
  - Removed predicates::is_block_macro() and is_block_modifier()
  - Removed lookup::is_block_macro()
  - Updated test_is_block_macro → test_body_macros + test_body_macro_names + test_body_requirement
- All 520 tests pass (1 new test added for BodyRequirement)

Stage Summary:
- Core parser refactor complete: flat parse + tree build replaces recursive block parsing
- Macro polymorphy bug FIXED: <<link "Go" "Room">> now correctly parsed as inline
  (Optional + no close tag → children: None)
- Block-list drift FIXED: tree builder uses catalog's BodyRequirement, no hardcoded lists
  in the parser
- <<timed>>, <<repeat>>, <<createplaylist>> now correctly paired by tree builder
  (BodyRequirement::Required)
- New pipeline: Phase 1 (flat parse) → Phase 1.5 (tree build) → Phase 2 (JS annotate) → Phase 3 (registry)
- Three divergent block-macro lists reduced to one: BodyRequirement in the catalog
- No regressions: all existing tests pass with same AST output

---
Task ID: 7
Agent: Main
Task: Audit parser_worklog.md against code and clean up remaining block_macro_names() drift

Work Log:
- Read parser_worklog.md, docs/worklog.md, docs/plan.md for context
- Cross-referenced all parser_worklog claims against actual code:
  - BodyRequirement enum in types.rs ✅
  - MacroClose variant in ast.rs ✅
  - close_name_span on AstNode::Macro ✅
  - Parser flat emission (no is_block_macro, no parse_block_body) ✅
  - Tree builder wired in parser/mod.rs ✅
  - predicates::is_block_macro() removed ✅
  - lookup::is_block_macro() removed ✅
  - All 520 tests pass ✅
- Found incomplete cleanup: classifiers::block_macro_names() still existed as a
  hardcoded list with 4 downstream consumers (syntax_detect, snippets, plugin trait, tests)
- Found drift in block_macro_names():
  - Missing: css, timed, repeat, createplaylist (Required in catalog but absent)
  - Incorrectly included: else, elseif, case, default (Never in catalog, are modifiers)
- Found bugs in downstream consumers:
  - snippets.rs line 88: redundant block_macro_names().contains(name) fallback caused
    else/case/default to get block snippets with phantom close tags
  - syntax_detect.rs line 141: locally hardcoded ["else", "elseif"] missing case/default
- Fixed all issues:
  - Replaced classifiers::block_macro_names() with body_macro_names() (catalog-derived)
  - Renamed FormatPlugin::block_macro_names() → body_macro_names()
  - Updated SugarCube plugin impl
  - Fixed snippets.rs: removed block_macro_names() import, simplified to body != Never
  - Fixed syntax_detect.rs: uses folding_modifier_names() from catalog instead of local list
  - Updated completion.rs and macros.rs (server handlers)
  - Updated tests: test_body_macros + test_body_macro_names verify catalog-derived list
- All 520 tests pass (0 failures)
- Updated parser_worklog.md with audit section and completion status

Stage Summary:
- block_macro_names() fully eliminated — replaced with catalog-derived body_macro_names()
- All downstream consumers now use BodyRequirement directly or via body_macro_names()
- syntax_detect.rs folding modifier list now uses folding_modifier_names() (no drift)
- snippets.rs no longer generates phantom close tags for structural modifiers
- macros.rs classify() uses mdef.body != BodyRequirement::Never directly
- Zero hardcoded block-macro name lists remain (except folding_modifier_names() which
  is a separate concept — structural subdivision, not body capability)

---
Task ID: 8
Agent: Main
Task: Comprehensive audit of Phases 1-3 + Task 6/7 + fix remaining hardcoded list drift

Work Log:
- Read worklog.md, plan.md, and all key implementation files
- Audited Phase 1 (Literals/Operators): matches plan exactly ✅
- Audited Phase 2 (Namespaces): working correctly, minor design deviation
  (skip-condition approach instead of plan's preferred Namespace-on-Variable
  approach for State.variables — functionally equivalent)
- Audited Phase 3 (Function/Widget): matches plan exactly ✅
- Audited Task 6 (Parser refactor): flat parse + tree builder correct ✅
- Audited Task 7 (Cleanup): block_macro_names fully eliminated ✅
- Found 5 remaining hardcoded-list drift issues:
  1. deprecated_macros() in lookup.rs — manual HashMap duplicated catalog data
  2. INLINE_JS_MACROS in ast.rs:767 — missing capture/unset (JS validation bug)
  3. INLINE_JS_MACROS in js_annotate.rs:230 — hardcoded, missing for/switch
  4. known_macro_names() in lookup.rs — hardcoded list duplicated catalog names
  5. dynamic_navigation_macros() in classifiers.rs — hardcoded list
- Fixed all 5 issues:
  - deprecated_macros() now derives from builtin_macros().filter(|m| m.deprecated)
  - Added inline_js_macro_names() to classifiers.rs (catalog-derived:
    Expression/Variable args + control-flow macros with undeclared JS args)
  - Replaced hardcoded INLINE_JS_MACROS in js_annotate.rs with
    inline_js_macro_names()
  - Replaced hardcoded INLINE_JS_MACROS in ast.rs with inline_js_macro_names()
    AND added $/_ fallback (previously missing entirely)
  - known_macro_names() now derives from builtin_macros().map(|m| m.name)
  - dynamic_navigation_macros() now derives from catalog's is_passage_ref
    + manual back/return entries
- Added 4 new tests: test_inline_js_macro_names,
  test_dynamic_navigation_macros_derived, test_known_macro_names_derived,
  test_deprecated_macros_derived
- All 524 tests pass (4 new, 0 failures)

Stage Summary:
- All hardcoded macro name lists now derive from the catalog (single source of truth)
- The only remaining hardcoded entries are:
  - folding_modifier_names() (else/elseif/case/default) — fixed SugarCube syntax rule
  - "back"/"return" in dynamic_navigation_macros() — navigate dynamically but have no
    passage arg in catalog
  - "if"/"elseif"/"for"/"switch"/"while" in inline_js_macro_names() — always-JS
    control-flow macros with undeclared args in catalog
- ast.rs collect_js_snippets now has the $/_ fallback that was previously missing,
  fixing a gap where macros like <<goto $target>> wouldn't get JS validation
- Phase 2 deviation noted but not changed: State.variables still emits Variable+Property
  instead of Namespace+Property (semantically less clean but functionally equivalent)

---
Task ID: 9
Agent: Main
Task: Phase 4 — Deprecated Macro Modifier

Work Log:
- Added `use crate::sugarcube::macros::deprecated_macros` import to token_builder.rs
- Modified `build_semantic_tokens()` macro name token emission to check deprecated_macros()
  and add `SemanticTokenModifier::Deprecated` when macro is deprecated
- Modified `build_diagnostics()` to emit Hint-severity diagnostics for deprecated macro usage
  with code "sc-deprecated" and the catalog's deprecation_message
- Added 5 integration tests:
  - sugarcube_deprecated_macro_token_modifier: <<click>> gets Deprecated modifier
  - sugarcube_non_deprecated_macro_no_modifier: <<link>> gets no Deprecated modifier
  - sugarcube_deprecated_macro_diagnostic: <<click>> gets sc-deprecated Hint diagnostic
  - sugarcube_display_deprecated_diagnostic: <<display>> gets both Deprecated modifier and diagnostic
  - sugarcube_set_not_deprecated: <<set>> gets no deprecated modifier or diagnostic
- cargo check passes, 529 tests pass (5 new, 0 failures)
- Updated plan.md progress tracker: Phase 4 → Complete

Stage Summary:
- Deprecated macros (click, display, remember, forget, setcss, settitle) now show strikethrough
  in editor via Deprecated semantic token modifier
- Deprecation hint diagnostics appear with catalog's replacement suggestion messages
- Catalog-derived: deprecated_macros() from builtin_macros().filter(|m| m.deprecated)

---
Task ID: 10
Agent: Main
Task: Phase 5 — Contextual Macro Parsing (capture_target, for_loop_vars)

Work Log:
- Added ForLoopVars struct to ast.rs with index_var and iterated_var VarRef fields
- Added capture_target: Option<VarRef> and for_loop_vars: Option<ForLoopVars> fields
  to AstNode::Macro variant
- Added parse_capture_target() function in macro_parser.rs:
  - Scans first $var or _var from <<capture>> args
  - Supports dot-notation property paths (e.g., <<capture $target.name>>)
  - Sets is_write: true (capture target is always a write)
- Added parse_for_loop_vars() function in macro_parser.rs:
  - Detects simplified iteration form by comma separator: <<for _i, $array>>
  - Returns None for C-style for loops (<<for _i to 0; _i lt 10; _i++>>)
  - Index var (_i) marked as is_temporary: true, is_write: true
  - Iterated var ($array) marked as is_write: false (read)
  - Supports dot-notation on iterated var (e.g., <<for _item, $player.inventory>>)
- Updated all AstNode::Macro construction sites:
  - macro_parser.rs: 2 sites (raw-body macro return + flat emission return)
  - tree_builder.rs: 4 sites (close-tag-paired, Never body, Optional body, Required body)
- Updated registry_populate.rs:
  - collect_var_ops_from_nodes() now extracts capture_target and for_loop_vars
  - Emits capture_target as VarAccessKind::Capture (complements JS annotation)
  - Emits for_loop_vars index_var as Write, iterated_var as Read
  - Both with deduplication against js_analysis (only emit if not already covered)
  - determine_macro_override() now accepts capture_target for precise Capture matching
- Added 6 unit tests in macro_parser.rs:
  - parse_capture_target: <<capture $target>> extracts capture_target
  - parse_capture_temp_var: <<capture _temp>> extracts temporary var
  - parse_for_loop_simplified: <<for _i, $items>> extracts for_loop_vars
  - parse_for_loop_c_style_no_for_loop_vars: C-style returns None
  - parse_for_loop_with_property_path: <<for _item, $player.inventory>> extracts property path
  - parse_capture_with_property_path: <<capture $target.name>> extracts property path
- cargo check passes (0 warnings in knot-formats), 535 tests pass (6 new, 0 failures)
- Updated plan.md progress tracker: Phase 5 → Complete

Stage Summary:
- <<capture $var>> now has structured capture_target field at AST level
- <<for _i, $array>> simplified form now has structured for_loop_vars field
- Both fields feed into registry population for accurate variable tracking
- capture_target enables VarAccessKind::Capture without relying on JS annotation heuristic
- for_loop_vars enables loop index var tracking even when oxc can't classify it
- C-style for loops correctly fall through to JS annotation (for_loop_vars = None)
- All existing tests pass with no regressions
