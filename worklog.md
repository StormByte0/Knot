---
Task ID: 1
Agent: Main
Task: Cross-check design docs vs code and implement architectural fixes

Work Log:
- Read all 3 design docs (ARCHITECTURE.md, PROMISES.md, HOOK_SYSTEM.md)
- Read all core files (lexer, parser, workspaceIndex, diagnosticEngine, etc.)
- Read all hook files (hookTypes, formatHooks, hookRegistry)
- Read all format adapters (fallback, sugarcube, harlowe)
- Read lspServer.ts and handler stubs
- Identified 12 architecture issues via cross-check
- Implemented fixes for all critical issues

Stage Summary:
- **hookTypes.ts**: Slimmed PassageType to Twine engine only (Story, Script, Stylesheet, Start, StoryData, Custom); trimmed MacroCategory to 8 universal values; removed DiagnosticRule enum entirely; kept MacroKind, MacroBodyStyle, FormatCapability
- **formatHooks.ts**: Added FormatASTNodeType (format-provided AST node types); added FormatDiagnosticRule (format-provided diagnostic rules); added FormatAdapterExport (standardized export shape); added CORE_DIAGNOSTIC_RULES; removed parseLinkSyntax alias
- **parser.ts**: Removed ASTNodeType enum from core; added getASTNodeTypes() method delegating to adapter; added DEFAULT_AST_NODE_TYPES for fallback
- **diagnosticEngine.ts**: Replaced DiagnosticRule enum with string-based rule IDs; uses CORE_DIAGNOSTIC_RULES + format-provided rules
- **workspaceIndex.ts**: Added customTypeId field to PassageEntry; added getPassagesByCustomType() method
- **lspServer.ts**: Fixed hardcoded `$ = story` sigil logic; removed `|` pipe SugarCube assumptions in rename; replaced parseLinkSyntax with resolveLinkBody
- **linkGraph.ts**: Added LinkKind.Custom to reachability traversal
- **formatAdapter.ts**: Removed (dead code)
- **formatRegistry.ts**: Uses provider metadata for detection, not hardcoded strings; registers via FormatAdapterExport
- **fallback/adapter.ts**: Updated to new interfaces; added AST node types; added standardized export
- **sugarcube/adapter.ts**: Updated MacroCategory to Custom + categoryDetail; PassageType.Custom + typeId; new diagnostic interface; added AST node types; removed parseLinkSyntax; added standardized export
- **harlowe/adapter.ts**: Same updates as SugarCube; 152 macro definitions updated to new MacroCategory
- TypeScript compilation: **ZERO ERRORS**

---
Task ID: 2
Agent: Main
Task: Design and implement standardized FormatModule export architecture

Work Log:
- Analyzed proposal: identified 6 good ideas and 8 bad/wrong ideas
- Designed refined FormatModule interface: capability bags, declarative data, enum-driven
- Created formats/_types.ts with FormatModule, MacroDef, BodyToken, SpecialPassageDef, etc.
- Rewrote sugarcube/adapter.ts → sugarcubeModule: FormatModule (all capability bags)
- Rewrote harlowe/adapter.ts → harloweModule: FormatModule (239 macros, new flags)
- Rewrote fallback/adapter.ts → fallbackModule: FormatModule (minimal, no bags)
- Rewrote formatRegistry.ts: lazy loading, O(1) alias resolution, detectFromStoryData()
- Updated core/parser.ts: FormatModule consumption, specialPassages lookup
- Updated core/workspaceIndex.ts: FormatRegistry instead of HookRegistry
- Updated core/diagnosticEngine.ts: format.diagnostics capability bag
- Updated lspServer.ts: FormatRegistry + FormatModule throughout
- Updated all handlers: FormatRegistry instead of HookRegistry
- Deprecated hookRegistry.ts and formatHooks.ts

Stage Summary:
- Full migration from IFormatProvider/HookRegistry to FormatModule/FormatRegistry
- Key improvements: capability bags, declarative special passages, declarative diagnostic rules, BodyToken with string typeIds, AST node types from formats
- Zero format bleed-through maintained
- Enum-driven data interchange preserved

---
Task ID: 3
Agent: Main
Task: Complete all story format files with full features, rules, and correct exports

Work Log:
- Completed SugarCube adapter: 1,349 lines, 70 macros, 24 snippets, 11 runtime globals
- Completed Harlowe adapter: 2,128 lines across 8 files (macros split into 6 category files + helpers + index), 257 macros
- Completed Chapbook adapter: 1,129 lines, ~28 inserts/modifiers, YAML front matter support
- Completed Snowman adapter: 460 lines, template blocks, JS API passage refs
- Completed Fallback adapter: 177 lines, basic [[ ]] link support
- All formats export standardized FormatModule with astNodeTypes, tokenTypes, lexBody, extractPassageRefs, resolveLinkBody, specialPassages, capability bags

Stage Summary:
- All 5 format modules complete and passing type checking
- Formats declare AST node types and token types; core uses them for tree building
- Macro categories cover: control, navigation, variable, output, interactive, dialog, audio, save, styling, utility, revision, system, live/timed, widget, form inputs, deprecated
- Harlowe macros split into 6 category files for maintainability (macros-basics, macros-data, macros-display, macros-interactive, macros-navigation, macros-advanced)

---
Task ID: 4
Agent: Main
Task: Build AST, parser, syntax/semantic analyzers, virtual docs, and workspace integration

Work Log:
- Created core/ast.ts (250 lines): ASTNode, ASTNodeData, DocumentAST, PassageGroup, visitor pattern (walkTree, walkTreeBreadthFirst, findDeepestNode, findAncestor), node creation helpers, tree utilities (countNodes, treeDepth, printTree)
- Created core/astBuilder.ts (740 lines): ASTBuilder class with 3 nesting strategies:
  - CloseTag (SugarCube): Stack-based <<if>>/<</if>> matching with child sibling tracking
  - Hook (Harlowe): Changer macro → hook [...] association with stack tracking
  - Inline (Chapbook/Snowman/Fallback): Flat token sequence, no nesting
  - Link node insertion with Text node splitting for precise range mapping
- Created core/syntaxAnalyzer.ts (430 lines): 7 structural checks:
  - Unclosed macros, mismatched close tags, invalid nesting (children/parents), missing arguments
  - Unclosed hooks (Harlowe), unclosed templates (Snowman), orphan close tags, duplicate passage names
  - Macro nesting stack builder for completion context
- Created core/semanticAnalyzer.ts (380 lines): 6 semantic checks:
  - Unknown macros (builtins + custom), deprecated macros, unknown variables, unknown passage refs
  - Variable scope (temp vars used before definition), custom macro resolution (widget/macro:)
  - Variable definition collection and custom macro definition collection
- Created core/virtualDocs.ts (290 lines): VirtualDocProvider with:
  - Virtual document URI scheme (tweedoc://), build/parse/validate utilities
  - Script passages → JavaScript, stylesheet passages → CSS
  - SugarCube <<script>>/<<style>> macro body extraction
  - Snowman template block extraction
  - Diagnostic remapping from virtual docs back to source Twee document
- Created core/astWorkspace.ts (230 lines): ASTWorkspace coordinator:
  - Single entry point: buildAndAnalyze() → full pipeline (AST → syntax → semantic → virtual docs)
  - Per-document analysis cache with version tracking
  - Symbol table population from AST data
  - Macro nesting stack access for completion context
  - Incremental invalidation on document change
- Updated core/diagnosticEngine.ts: Added Phase 3 AST-based diagnostics (prefers AST results when available, falls back to regex-based Phase 1/2)
- Updated core/index.ts: All new exports (ast, astBuilder, syntaxAnalyzer, semanticAnalyzer, virtualDocs, astWorkspace)
- Updated lspServer.ts: Integrated ASTWorkspace into document lifecycle (open/change/close), fixed handleDefinition bug (undefined `text` variable), reindexAll clears and rebuilds AST cache
- Fixed chapbook/adapter.ts: fmMatch.index possibly undefined type error
- TypeScript compilation: **ZERO ERRORS** on server project

Stage Summary:
- Complete analysis pipeline: RawPassage → BodyToken[] → AST → SyntaxAnalysis → SemanticAnalysis → VirtualDocs
- Format-agnostic AST builder uses macroBodyStyle to select nesting strategy
- Syntax analyzer catches 7 categories of structural errors
- Semantic analyzer performs cross-passage variable/macro reference validation
- Virtual documents enable embedded JS/CSS language features
- DiagnosticEngine Phase 3 uses AST results when available
- All changes maintain the "core never imports from formats/" invariant
