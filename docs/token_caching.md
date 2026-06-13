# Project Plan: Rebuild ver_3 Features on f65d6e2 Baseline

> **Interrogation Status**: This plan was aggressively interrogated against the actual
> codebase at both f65d6e2 and ver_3 HEAD. 12 discrepancies were found and corrected.
> See the "Interrogation Corrections" section at the end of each phase for details.

## Objective

Revert to commit f65d6e2 and re-implement the features from the 5 subsequent commits,
with fine-grained control logic at every step to prevent the unrecoverable runtime
breakage that occurred in the current ver_3.

---

## Root Cause Analysis: What Broke ver_3

The 5 commits after f65d6e2 were applied in an order that created a **dependency gap**:
architectural changes that required new infrastructure were committed BEFORE that
infrastructure existed. Specifically:

1. **Commit a38133f** (`FormatPluginMut`, removed `parking_lot::RwLock`) made ALL
   parsing require the server write lock. But `semantic_tokens_full` still tried to
   re-parse under a read lock → **deadlock or empty tokens**.

2. **Commit 3c29c06** changed `initialized` to run `index_workspace` inline instead
   of spawning a task. This blocked the `initialized` handler from returning until
   indexing completed, which prevented VS Code from receiving the response and
   processing subsequent messages → **server appears frozen**.

3. **Commit 1844839** added `clientReady` handshake to fix the startup race, but
   the provider polling (VariableFlow, Profile) started BEFORE `clientReady` was
   sent (in `setClient()` calls before `registerNotifications()`), sending requests
   to an unready server → **early request failures cascading into stuck UI**.

4. The `format_switch_in_progress` flag was only checked in `did_open`, not
   `did_change` → **unsuppressed refreshes during cascade caused O(N²) token
   request flood that overwhelmed the server**.

5. `remove_passage_from_registries` calls BOTH `remove_passage` AND `remove_file`,
   nuking all passages in a file when only one was supposed to be removed →
   **silent data loss on incremental re-parse**.

### The Fix: Correct Ordering

The solution is to apply changes in **dependency order**, where each phase's
prerequisites are already in place before the phase begins:

```
Infrastructure first → then architectural changes → then features
```

---

## Phase Execution Contract

Every phase MUST satisfy these invariants before being considered complete:

1. **`cargo check` passes** with zero errors
2. **`cargo test` passes** for all affected crates
3. **Extension starts** without errors in the VS Code developer console
4. **A .tw file opens** and shows semantic highlighting (passage headers, macros, variables, links)
5. **The passage graph** renders correctly in the Story Map
6. **Variable tracking** shows entries in the Variable Flow panel
7. **File switching** preserves highlighting (no permanent token loss)
8. **Edit → save** triggers correct re-parse (no stale diagnostics)

If any invariant fails, the phase is NOT complete. Do not proceed to the next phase.

---

## Phase 1: Baseline — Reset to f65d6e2 + Bug Fixes

### Goal

Establish a stable, validated baseline at f65d6e2 with its known bugs fixed.

### 1.1 Hard Reset

```bash
git checkout ver_3
git checkout -b ver_3_rebuild
git reset --hard f65d6e2
```

### 1.2 Verify Baseline Works

- `cargo check` passes
- `cargo test` passes (for crates/core, crates/formats, crates/server)
- Extension starts in VS Code, opens a .tw file, shows highlighting

**Record**: What specific bugs are visible at f65d6e2? (empty file_uri, duplicate
variables, etc.)

### 1.3 Fix: `parse_single()` Empty `file_uri`

**Problem**: `parse_single()` in `parse_pipeline.rs` constructs a `ClassifiedPassage`
with `file_uri: ""`. When registry entries are recorded, they get `file_uri: ""`,
which doesn't match the entries created by `parse_full()` (which uses the real URI).
This means incremental re-parse can't correctly remove old entries.

**Change**: Add `file_uri: &str` parameter to `parse_single()` AND update the
`FormatPlugin::parse_passage()` trait method.

**File**: `crates/formats/src/sugarcube/parse_pipeline.rs`

**Before** (4 params — plan originally said 3, but `passage_tags` was missed):
```rust
pub(super) fn parse_single(
    plugin: &SugarCubePlugin,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
) -> Option<Passage>
```

**After** (5 params — `file_uri` added after `passage_text`):
```rust
pub(super) fn parse_single(
    plugin: &SugarCubePlugin,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
    file_uri: &str,           // ← ADDED
) -> Option<Passage>
```

**And in the ClassifiedPassage construction inside `parse_single`**:
```rust
file_uri: file_uri.to_string(),  // was: String::new()
```

**Also update the trait method** in `crates/formats/src/plugin.rs`:
```rust
// Before:
fn parse_passage(&self, passage_name: &str, passage_tags: &[String], passage_text: &str) -> Option<Passage>;

// After:
fn parse_passage(&self, passage_name: &str, passage_tags: &[String], passage_text: &str, file_uri: &str) -> Option<Passage>;
```

**Callers that must be updated**:
- `SugarCubePlugin::parse_passage()` in `sugarcube/mod.rs` — must pass `uri.as_ref()`
- `HarlowePlugin::parse_passage()` in `harlowe/mod.rs` — add `file_uri` param (can ignore)
- `ChapbookPlugin::parse_passage()` in `chapbook/mod.rs` — add `file_uri` param (can ignore)
- `SnowmanPlugin::parse_passage()` in `snowman/mod.rs` — add `file_uri` param (can ignore)
- Any server handler that calls `plugin.parse_passage()` — must pass the URI

**Control logic**: After this change, `registry.remove_passage()` in `parse_single`
will correctly match entries by `file_uri`. Verify by:
1. Open a multi-passage file
2. Edit one passage
3. Check that only that passage's registry entries are removed/re-added
4. Check that other passages' entries are preserved

### 1.4 Verify: `remove_file` Responsibility (NO CHANGE NEEDED)

**Original claim**: Both callers AND `parse_full()` call `registry.remove_file()`,
creating a double-removal.

**Actual state at f65d6e2** (verified by code audit):
- `parse_full()` in `parse_pipeline.rs:33` calls `registry.remove_file(uri.as_ref())` ✓
- `did_open` does NOT call `remove_file_from_registries` before parse ✓
- `did_change` does NOT call `remove_file_from_registries` before parse ✓
- `indexing.rs` Pass 2 does NOT call `remove_file_from_registries` ✓
- `did_change_watched_files` DELETED case calls `remove_file_from_registries` — but this
  is AFTER removing the document (cleanup), not before a parse ✓

**Conclusion**: There is NO double-removal at f65d6e2. `parse_full()` is already the
SOLE owner of `remove_file` before re-population. The current single-point design is
correct and should NOT be changed.

**DO NOT** move `remove_file` out of `parse_full()` — that would SPREAD responsibility
to multiple call sites, increasing the risk of missing one.

**Control logic** (document for future maintainers):
- `parse_full()` = "remove file's old data, then parse and populate" — owns `remove_file`
- `parse_single()` = "remove passage's old data, then parse and populate" — owns `remove_passage`
- Callers = "call parse_full/parse_single, don't touch registries"
- Exception: `did_change_watched_files` DELETED case calls `remove_file_from_registries`
  as cleanup AFTER removing the document (not before a parse)

### 1.5 Fix: `remove_passage_from_registries` Double-Removal Bug

**Problem**: `SugarCubePlugin::remove_passage_from_registries()` calls BOTH
`self.registry.remove_passage(passage_name)` AND `self.registry.remove_file(file_uri)`.
The second call nukes ALL passages in the file, not just the one specified.

**Change**: Remove the `remove_file` call from `remove_passage_from_registries`.

**File**: `crates/formats/src/sugarcube/mod.rs`

**Before**:
```rust
fn remove_passage_from_registries(&self, passage_name: &str, file_uri: &str) {
    self.registry.remove_passage(passage_name);
    self.registry.remove_file(file_uri);  // ← BUG: removes ALL passages in file
}
```

**After**:
```rust
fn remove_passage_from_registries(&self, passage_name: &str, _file_uri: &str) {
    self.registry.remove_passage(passage_name);  // only remove the specified passage
}
```

**Control logic**: `remove_passage_from_registries` now only removes ONE passage.
`remove_file_from_registries` removes ALL passages in a file. No overlap.

### 1.6 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] Extension starts
- [ ] .tw file opens with highlighting
- [ ] Edit a passage → re-parse works without data loss in other passages
- [ ] Variable Flow panel shows correct entries (no duplicates from empty file_uri)

---

## Phase 2: Semantic Token Caching

### Goal

Cache semantic tokens at parse time so `semantic_tokens_full` never needs to re-parse.

### Why This Phase Is First

This is the most critical infrastructure for all subsequent phases. Without it:

- `FormatPluginMut` (Phase 4) would require `semantic_tokens_full` to acquire
  the write lock, causing deadlock
- Format switch cascade protection (Phase 3) can't preserve tokens across
  didClose+didOpen without a cache

### 2.1 Add Token Cache to ServerStateInner

**File**: `crates/server/src/state.rs`

**Add field**:
```rust
pub struct ServerStateInner {
    // ... existing fields ...
    semantic_tokens: HashMap<Url, Vec<SemanticToken>>,  // ← ADDED
}
```

**Initialize** in `ServerStateInner::new()`:
```rust
semantic_tokens: HashMap::new(),
```

**Design note**: `SemanticToken` here is the format-plugin type
(`crate::formats::plugin::SemanticToken`), NOT the LSP type. The conversion
to LSP wire format happens in `semantic_tokens_full`.

### 2.2 Store Tokens at Every Parse Point

**Invariant**: EVERY call to `parse_with_format_plugin()` MUST be followed by
`semantic_tokens.insert(uri, tokens)`. No exceptions.

**File**: `crates/server/src/handlers/sync.rs`

**In `did_open`** (after parse):
```rust
inner.semantic_tokens.insert(uri.clone(), parse_result.tokens.clone());
```

**In `did_change`** (after parse):
```rust
inner.semantic_tokens.insert(uri.clone(), parse_result.tokens.clone());
```

**In `did_close`**: DO NOT remove tokens. Preserving them is critical for
the format-switch cascade (Phase 3). Stale tokens are better than no tokens —
VS Code will re-request after a refresh.

**File**: `crates/server/src/handlers/helpers/indexing.rs`

**In Pass 2 loop** (after parse):
```rust
inner.semantic_tokens.insert(uri.clone(), parse_result.tokens.clone());
```

**In post-Pass 2 re-parse** (after parse):
```rust
inner.semantic_tokens.insert(uri.clone(), parse_result.tokens.clone());
```

### 2.3 Change `semantic_tokens_full` to Read from Cache

**File**: `crates/server/src/handlers/semantic.rs`

**Before** (f65d6e2 behavior — re-parses on every request):
```rust
async fn semantic_tokens_full(&self, uri: Url) -> JsonResult<SemanticTokensResult> {
    let inner = self.inner.read().await;
    // ... re-parse the document to get tokens ...
}
```

**After** (cache-first):
```rust
async fn semantic_tokens_full(&self, uri: Url) -> JsonResult<SemanticTokensResult> {
    let inner = self.inner.read().await;

    match inner.semantic_tokens.get(&uri) {
        Some(tokens) => {
            // Convert format-plugin SemanticToken → LSP SemanticToken
            let lsp_tokens = convert_semantic_tokens(tokens, &inner, &uri);
            Ok(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data: encode_semantic_tokens(&lsp_tokens),
            }))
        }
        None => {
            // Return LSP null — VS Code will re-request after a refresh.
            // Do NOT return empty SemanticTokens — VS Code caches that as
            // "no tokens" and won't re-request until the next refresh.
            Ok(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data: vec![],
            }))
        }
    }
}
```

**CRITICAL CONTROL LOGIC**:

The `None` case returns empty `data` but as a `SemanticTokens` struct (not `None`
at the JSON level). At the f65d6e2 stage, this is acceptable because:
1. The format switch cascade protection (Phase 3) hasn't been added yet
2. The `did_close` preserving tokens (Phase 2.2) ensures tokens survive cascades
3. If tokens are truly missing (never parsed), empty is correct

In Phase 3, we'll upgrade this to return JSON `null` instead of empty, which
tells VS Code "tokens not available, please re-request after refresh."

**IMPORTANT**: The function `convert_semantic_tokens` ALREADY EXISTS in `semantic.rs`
(line 66-120 at f65d6e2). It converts format-plugin `SemanticToken` → LSP `SemTok`
with byte-to-line/char conversion and important clamping/safety logic. **DO NOT
recreate this function** — reuse the existing one.

**IMPORTANT CONCURRENCY NOTE**: The current f65d6e2 `semantic_tokens_full` does a
FULL RE-PARSE on every request under a READ lock on `ServerStateInner`. But
`parse_full()` internally calls `registry.remove_file()` and `populate_registries_from_ast()`,
which WRITE to the `SugarCubeRegistry` sub-registries via `parking_lot::RwLock`. This
means a "read" operation on the server state is actually triggering hidden writes to
sub-registries. This is architecturally unsound — the token cache fix eliminates this
concurrency correctness problem entirely.

### 2.4 Add `tokio::task::yield_now()` to Indexing Pass 2

**Problem**: At f65d6e2, the Pass 2 loop acquires/releases the write lock in a
tight loop without yielding. This starves other tokio tasks (especially `did_open`)
that need the lock.

**File**: `crates/server/src/handlers/helpers/indexing.rs`

**In Pass 2 loop** (after `drop(inner)`):
```rust
drop(inner);
tokio::task::yield_now().await;  // ← ADDED: give other tasks a chance
```

**Control logic**: This ensures that between each file parse, the tokio runtime
can process pending tasks (did_open, did_change, etc.). Without it, indexing
a large workspace would block ALL other operations until complete.

### 2.5 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] Extension starts
- [ ] .tw file opens with highlighting (tokens served from cache, not re-parsed)
- [ ] Switch between files → highlighting persists in each file
- [ ] Edit file → save → highlighting updates correctly (cache refreshed)
- [ ] Open a second file while first is still being indexed → no deadlock
- [ ] Close a file → reopen it → highlighting still works (cache preserved)

---

## Phase 3: Format Switch Cascade Protection

### Goal

Prevent semantic token loss during the formatDetected → language switch cascade,
and eliminate the startup race condition where notifications arrive before
handlers are registered.

### Why This Phase Before FormatPluginMut

`FormatPluginMut` (Phase 4) will make ALL parsing require the write lock. During
a format switch cascade, N didClose+didOpen pairs arrive in quick succession,
each needing the write lock. Without cascade protection, the server would:

1. Process didOpen #1 → parse → send token refresh
2. Process didOpen #2 → parse → send token refresh (cancels #1)
3. ... N times → O(N²) token requests that overwhelm VS Code

With cascade protection, the server suppresses all intermediate refreshes and
sends ONE refresh after the cascade completes.

### 3.1 Add `knot/clientReady` Handshake (Server Side)

**File**: `crates/server/src/state.rs`

**Add fields**:
```rust
pub struct ServerState {
    pub client: Client,                              // ← ALREADY EXISTS at f65d6e2
    pub inner: Arc<RwLock<ServerStateInner>>,        // ← MUST CHANGE from RwLock to Arc<RwLock>
    pub client_ready: Arc<Notify>,                   // ← ADDED
    pub shutting_down: AtomicBool,                   // ← ALREADY EXISTS at f65d6e2
}

**CRITICAL**: At f65d6e2, `inner` is `RwLock<ServerStateInner>`, NOT `Arc<RwLock<...>>`.
`RwLock` from `tokio::sync` is `Send + Sync` but NOT `Clone`. Since `tokio::spawn`
requires `'static + Send` futures, we MUST wrap `inner` in `Arc` to clone it into
the spawned task. This is a prerequisite for Phase 3's spawned indexing task.
```

**In `ServerState::new()`**:
```rust
inner: Arc::new(RwLock::new(ServerStateInner::new(/* ... */))),  // ← wrap in Arc
client_ready: Arc::new(Notify::new()),
```

**Also update ALL handler methods** that access `self.inner` — they currently use
`self.inner.read().await` and `self.inner.write().await`, which still works with
`Arc<RwLock<...>>` (Arc derefs to the inner type). No handler code changes needed
for this part.

**File**: `crates/server/src/handlers/lifecycle.rs`

**Change `initialized` handler**:

**Before** (f65d6e2 — inline indexing):
```rust
async fn initialized(&self) {
    helpers::index_workspace(&self.inner, &self.client).await;
}
```

**After** (spawned task with clientReady gate):
```rust
async fn initialized(&self) {
    let inner = self.inner.clone();
    let client = self.client.clone();
    let client_ready = self.client_ready.clone();

    tokio::spawn(async move {
        // WAIT for the extension to confirm it's ready.
        // This eliminates the race where the server sends formatDetected
        // before the extension has registered notification handlers.
        client_ready.notified().await;

        helpers::index_workspace(&inner, &client).await;
    });
}
```

**Add `knot_client_ready` handler**:

**File**: `crates/server/src/lib.rs` — register the custom method following the
existing pattern:
```rust
// In the LspService::build() chain:
.custom_method("knot/clientReady", ServerState::knot_client_ready)
.custom_method("knot/formatSwitchComplete", ServerState::knot_format_switch_complete)
```

**File**: `crates/server/src/handlers/lifecycle.rs` — implement as `impl ServerState`:
```rust
pub async fn knot_client_ready(&self) -> Result<KnotClientReadyResponse, tower_lsp::jsonrpc::Error> {
    self.client_ready.notify_one();
    Ok(KnotClientReadyResponse { acknowledged: true })
}
```

**File**: `crates/server/src/lsp_ext.rs` — define params/response types:
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyParams {}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyResponse {
    pub acknowledged: bool,
}
```

**File**: `crates/server/src/lsp_ext.rs`:
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyParams {}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyResponse {
    pub acknowledged: bool,
}
```

**Control logic**:

The `Notify` primitive is single-use-per-wait. `client_ready.notified().await`
blocks the spawned task until `notify_one()` is called. Once notified, the task
proceeds with indexing. If the extension never sends `clientReady`, the server
will never index — this is intentional (no data is better than corrupted data
from race conditions). A 30-second safety timeout could be added as a fallback:

```rust
tokio::select! {
    _ = client_ready.notified() => {},
    _ = tokio::time::sleep(Duration::from_secs(30)) => {
        tracing::warn!("clientReady not received after 30s, starting indexing anyway");
    }
}
```

### 3.2 Add `format_switch_in_progress` Flag

**File**: `crates/server/src/state.rs`

**Add field**:
```rust
pub struct ServerStateInner {
    // ... existing fields ...
    format_switch_in_progress: bool,  // ← ADDED
}
```

**Initialize**:
```rust
format_switch_in_progress: false,
```

### 3.3 Debounced Semantic Token Refresh

**File**: `crates/server/src/state.rs`

**Add fields**:
```rust
pub struct ServerState {
    // ... existing fields ...
    semantic_refresh_pending: Arc<AtomicBool>,  // ← ADDED
}
```

**Add method**:
```rust
impl ServerState {
    pub async fn schedule_semantic_token_refresh(&self, client: &Client) {
        if self.semantic_refresh_pending.compare_exchange(
            false, true, Ordering::Relaxed, Ordering::Relaxed
        ).is_err() {
            return; // refresh already pending
        }

        let pending = self.semantic_refresh_pending.clone();
        let client = client.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            pending.store(false, Ordering::Relaxed);
            let _ = client.send_request::<request::SemanticTokensRefresh>(()).await;
        });
    }
}
```

**Control logic**: The `compare_exchange` ensures only ONE debounce timer is
active at a time. Subsequent calls within 150ms are coalesced. This prevents
the O(N²) token request flood during format switch cascades.

### 3.4 Guard `did_open` and `did_change` During Format Switch

**File**: `crates/server/src/handlers/sync.rs`

**In `did_open`** — after setting `format_switch_in_progress = true` and sending
`formatDetected`:
```rust
// After sending formatDetected:
inner.format_switch_in_progress = true;
drop(inner);

// DON'T send semantic token refresh here — the cascade will send
// formatSwitchComplete when done, which triggers a single refresh
```

**In `did_open`** — when format switch is NOT happening, use debounced refresh:
```rust
if !inner.format_switch_in_progress {
    state.schedule_semantic_token_refresh(&client).await;
}
```

**In `did_change`** — same check:
```rust
if structure_changed && !inner.format_switch_in_progress {
    state.schedule_semantic_token_refresh(&client).await;
}
```

**CRITICAL**: This fixes the bug from the original commits where `did_change`
didn't check `format_switch_in_progress`.

### 3.5 Add `knot/formatSwitchComplete` Handshake

**File**: `crates/server/src/lsp_ext.rs`:
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatSwitchCompleteParams {
    pub workspace_uri: String,
    pub switched_count: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatSwitchCompleteResponse {
    pub acknowledged: bool,
}
```

**File**: `crates/server/src/handlers/lifecycle.rs`:
```rust
async fn knot_format_switch_complete(
    &self,
    params: KnotFormatSwitchCompleteParams,
) -> JsonResult<KnotFormatSwitchCompleteResponse> {
    {
        let mut inner = self.inner.write().await;
        inner.format_switch_in_progress = false;
    }
    // Send ONE unified refresh now that the cascade is complete
    self.schedule_semantic_token_refresh(&self.client).await;
    Ok(KnotFormatSwitchCompleteResponse { acknowledged: true })
}
```

**Safety timeout**: Also add a 2-second timeout that clears the flag if
`formatSwitchComplete` is never received (e.g., extension crash):

```rust
// In the indexing code, after sending formatDetected:
let inner_clone = self.inner.clone();
let client_clone = self.client.clone();
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut inner = inner_clone.write().await;
    if inner.format_switch_in_progress {
        inner.format_switch_in_progress = false;
        drop(inner);
        // Send refresh as fallback
        let _ = client_clone.send_request::<request::SemanticTokensRefresh>(()).await;
    }
});
```

### 3.6 Upgrade `semantic_tokens_full` to Return JSON `null` When No Cache

**File**: `crates/server/src/handlers/semantic.rs`

**Before** (Phase 2 behavior — returns empty SemanticTokens):
```rust
None => Ok(SemanticTokensResult::Tokens(SemanticTokens {
    result_id: None,
    data: vec![],
}))
```

**After** (returns JSON null):
```rust
None => Ok(SemanticTokensResult::None)
// This tells VS Code "tokens not available yet, re-request after refresh"
// VS Code will re-request after receiving workspace/semanticTokens/refresh
```

**Control logic**: With the `format_switch_in_progress` flag + debounced refresh
+ JSON null, the flow is:
1. Tokens requested during cascade → `None` → VS Code knows tokens aren't ready
2. Cascade completes → `formatSwitchComplete` → ONE refresh
3. VS Code re-requests tokens → cache is now populated → tokens served

### 3.7 Extension-Side Changes

**File**: `extensions/vscode/src/extension.ts`

**Add after `registerNotifications()` (line ~188)**:
```typescript
// Signal to the server that all notification handlers are registered.
// The server waits for this before starting indexing, ensuring that
// formatDetected and indexProgress notifications won't be dropped.
try {
    const response = await client.sendRequest('knot/clientReady', {});
    console.log('[knot] clientReady acknowledged:', response);
} catch (e) {
    console.warn('[knot] clientReady failed (server may be older version):', e);
}
```

**File**: `extensions/vscode/src/notifications.ts`

**Change `formatDetected` handler**:
```typescript
// Collect all switch promises
const switchPromises: Thenable<void>[] = [];
for (const docUri of params.document_uris) {
    const doc = vscode.workspace.textDocuments.find(d => d.uri.toString() === docUri);
    if (doc && doc.languageId !== languageId) {
        const p = vscode.languages.setTextDocumentLanguage(doc, languageId);
        switchPromises.push(p);
    }
}

// Wait for ALL switches to settle (not just resolve — allSettled handles failures)
Promise.allSettled(switchPromises).then(async () => {
    // Signal completion to the server — it will clear the format_switch_in_progress
    // flag and send ONE unified semantic token refresh
    try {
        await client.sendRequest('knot/formatSwitchComplete', {
            workspace_uri: params.workspace_uri,
            switched_count: switchPromises.length,
        });
    } catch (e) {
        console.warn('[knot] formatSwitchComplete failed:', e);
    }
    deps.refreshDecorations();
});
```

**Change `refreshSemanticTokens` handler** — remove `editor.action.semanticTokens.refresh`
(the server now sends `workspace/semanticTokens/refresh` which triggers it automatically):
```typescript
knot_refreshSemanticTokens: (params: any) => {
    // The server's workspace/semanticTokens/refresh already triggers
    // VS Code's built-in token refresh. We only need to refresh decorations.
    deps.refreshDecorations();
},
```

**File**: `extensions/vscode/src/types.ts`

**Add types**:
```typescript
export interface KnotClientReadyParams {}
export interface KnotClientReadyResponse { acknowledged: boolean }
export interface KnotFormatSwitchCompleteParams {
    workspace_uri: string;
    switched_count: number;
}
export interface KnotFormatSwitchCompleteResponse { acknowledged: boolean }
```

### 3.8 Already-Open File Handling in Indexing

**File**: `crates/server/src/handlers/helpers/indexing.rs`

**In Pass 2 loop** — skip files that are already open in the editor:
```rust
// Before parsing, check if this file is already open in an editor.
// If so, skip it here — it was already parsed in did_open, and we'll
// re-parse it with the correct format after Pass 2 completes.
if already_open.contains(uri) {
    continue;
}
```

**After Pass 2** — re-parse all editor-open documents with the resolved format:
```rust
// Re-parse editor-open documents with the correct format.
// This ensures format resolution from StoryData is applied to
// documents that were opened before the format was detected.
for (uri, text, version) in &editor_open_docs {
    let mut inner = inner.write().await;
    let format = inner.workspace.resolve_format(&uri);
    let (doc, parse_result) = helpers::parse_with_format_plugin(
        &inner.format_registry, uri, text, format, *version,
    );
    inner.workspace.insert_document(uri.clone(), doc);
    inner.semantic_tokens.insert(uri.clone(), parse_result.tokens.clone());
    drop(inner);
    tokio::task::yield_now().await;
}
```

### 3.9 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] Extension starts
- [ ] Server logs show "waiting for clientReady" → "clientReady received" → indexing starts
- [ ] Open a project with StoryData → format detected → language IDs switch → highlighting remains intact
- [ ] Rapid file switching during indexing → no deadlock, no permanent token loss
- [ ] `formatSwitchComplete` is sent after language switches complete
- [ ] Semantic token refresh is debounced (only ONE refresh after cascade)
- [ ] Close all files → reopen → highlighting works (cache preserved through did_close)

---

## Phase 4: FormatPluginMut — Replace parking_lot::RwLock

### Goal

Remove `parking_lot::RwLock` from all format registries. Replace interior
mutability with explicit `&mut self` methods. The server's `tokio::RwLock`
becomes the SOLE synchronization mechanism.

### Why This Phase Is Safe Now

After Phases 2 and 3:
- Semantic tokens are cached (Phase 2) → `semantic_tokens_full` doesn't need
  to parse → doesn't need `&mut FormatRegistry`
- Format switch cascade is protected (Phase 3) → no O(N²) refresh flood
- `clientReady` handshake (Phase 3) → no startup race condition
- `yield_now()` in indexing (Phase 2.4) → no starvation during indexing

These were the three scenarios where the old `&self + RwLock` architecture was
"needed" — and they're now handled by better mechanisms.

### 4.1 Create `FormatPluginMut` Trait

**File**: `crates/formats/src/plugin.rs`

```rust
/// Mutable operations on a format plugin.
///
/// All methods take `&mut self`, meaning the caller MUST hold exclusive access
/// to the plugin. In the server, this means holding the write lock on
/// ServerStateInner.
///
/// Read-only operations remain on the `FormatPlugin` trait with `&self`.
pub trait FormatPluginMut: FormatPlugin {
    /// Parse an entire file and return structured results.
    /// The plugin MUST call `registry.remove_file(uri)` before populating.
    fn parse_mut(&mut self, uri: &Url, text: &str) -> ParseResult;

    /// Re-parse a single passage incrementally.
    /// The plugin MUST call `registry.remove_passage(name, uri)` before populating.
    fn parse_passage_mut(&mut self, uri: &Url, text: &str, passage_name: &str) -> Option<Passage>;

    /// Remove all registry entries for a file.
    fn remove_file_from_registries(&mut self, file_uri: &str);

    /// Remove all registry entries for a single passage.
    fn remove_passage_from_registries(&mut self, passage_name: &str, file_uri: &str);
}
```

### 4.2 Remove RwLock from SugarCubeRegistry

**File**: `crates/formats/src/sugarcube/registries/mod.rs`

**Before**:
```rust
pub struct SugarCubeRegistry {
    variables: RwLock<VariableTree>,
    custom_macros: RwLock<CustomMacroRegistry>,
    functions: RwLock<FunctionRegistry>,
    templates: RwLock<TemplateRegistry>,
}

// Read accessors return guards:
pub fn variables(&self) -> RwLockReadGuard<'_, VariableTree> { self.variables.read() }
pub fn variables_mut(&self) -> RwLockWriteGuard<'_, VariableTree> { self.variables.write() }
```

**After**:
```rust
pub struct SugarCubeRegistry {
    variables: VariableTree,        // no RwLock
    custom_macros: CustomMacroRegistry,
    functions: FunctionRegistry,
    templates: TemplateRegistry,
}

// Read accessors return references:
pub fn variables(&self) -> &VariableTree { &self.variables }
pub fn variables_mut(&mut self) -> &mut VariableTree { &mut self.variables }

// Multi-registry write access (for populate_registries_from_unified_ast):
pub fn definition_registries_mut(&mut self) -> (&mut CustomMacroRegistry, &mut FunctionRegistry, &mut TemplateRegistry) {
    (&mut self.custom_macros, &mut self.functions, &mut self.templates)
}
```

**Control logic**: The borrow checker now enforces what `RwLock` used to enforce
at runtime. You cannot have `&self` and `&mut self` simultaneously. The compiler
will catch any code that tries to read while writing.

### 4.3 Implement FormatPluginMut for SugarCubePlugin

**File**: `crates/formats/src/sugarcube/mod.rs`

```rust
impl FormatPluginMut for SugarCubePlugin {
    fn parse_mut(&mut self, uri: &Url, text: &str) -> ParseResult {
        parse_pipeline::parse_full(self, uri, text)  // now takes &mut SugarCubePlugin
    }

    fn parse_passage_mut(&mut self, uri: &Url, text: &str, passage_name: &str) -> Option<Passage> {
        parse_pipeline::parse_single(self, passage_name, text, uri.as_ref())
    }

    fn remove_file_from_registries(&mut self, file_uri: &str) {
        self.registry.remove_file(file_uri);
    }

    fn remove_passage_from_registries(&mut self, passage_name: &str, _file_uri: &str) {
        self.registry.remove_passage(passage_name);  // only passage, NOT file
    }
}
```

### 4.4 Change `parse_pipeline::parse_full` to Take `&mut SugarCubePlugin`

**File**: `crates/formats/src/sugarcube/parse_pipeline.rs`

**Before**:
```rust
pub(super) fn parse_full(plugin: &SugarCubePlugin, uri: &Url, text: &str) -> ParseResult
```

**After**:
```rust
pub(super) fn parse_full(plugin: &mut SugarCubePlugin, uri: &Url, text: &str) -> ParseResult
```

**And change all internal registry access from**:
```rust
let registry = plugin.registry();  // returns RwLockReadGuard or RwLockWriteGuard
```
**to**:
```rust
let registry = plugin.registry_mut();  // returns &mut SugarCubeRegistry
```

### 4.5 Update FormatRegistry Container

**File**: `crates/formats/src/plugin.rs` (NOT types.rs — FormatRegistry lives in plugin.rs)

**Update `FormatRegistry` struct** to store `Box<dyn FormatPluginMut>` and provide
both read and write access:
```rust
pub struct FormatRegistry {
    plugins: Vec<Box<dyn FormatPluginMut>>,  // ← stores FormatPluginMut, not FormatPlugin
}

impl FormatRegistry {
    // Read-only access — used by handlers under the server's read lock
    pub fn get(&self, format: &StoryFormat) -> Option<&dyn FormatPlugin> {
        self.plugins.iter().find(|p| p.format() == *format).map(|b| b.as_ref())
    }

    // Mutable access — used by handlers under the server's write lock
    pub fn get_mut(&mut self, format: &StoryFormat) -> Option<&mut dyn FormatPluginMut> {
        self.plugins.iter_mut().find(|p| p.format() == *format).map(|b| b.as_mut())
    }
}
```

**CRITICAL**: `FormatPluginMut: FormatPlugin` means every `FormatPluginMut` is also
a `FormatPlugin`. So `Box<dyn FormatPluginMut>` can produce both `&dyn FormatPlugin`
(via `as_ref()`) and `&mut dyn FormatPluginMut` (via `as_mut()`). We do NOT need a
separate `FormatRegistryMut` trait — `FormatRegistry` provides both access patterns.

**Also REMOVE `fn parse(&self, ...)` from the `FormatPlugin` trait.** In ver_3, this
method was completely removed — all parsing goes through `FormatPluginMut::parse_mut()`.
All callers of `plugin.parse()` must be updated to `plugin.parse_mut()`.

### 4.6 Update All Server Handlers

**Invariant**: Handlers that ONLY read data (completion, hover, navigation,
semantic_tokens_full) use `format_registry.get()` with a READ lock.
Handlers that MUTATE data (did_open, did_change, indexing) use
`format_registry.get_mut()` with a WRITE lock.

**File**: `crates/server/src/handlers/helpers/parsing.rs`

**Before**:
```rust
pub(crate) fn parse_with_format_plugin(
    registry: &FormatRegistry,  // immutable
    ...
) -> (Document, ParseResult) {
    let plugin = registry.get(&format);
    plugin.parse(uri, text)  // &self
}
```

**After**:
```rust
pub(crate) fn parse_with_format_plugin(
    registry: &mut FormatRegistry,  // mutable
    ...
) -> (Document, ParseResult) {
    let plugin = registry.get_mut(&format);
    plugin.parse_mut(uri, text)  // &mut self
}
```

**Every caller of `parse_with_format_plugin` must now hold the WRITE lock.**
This is correct because they already hold it (did_open, did_change, indexing
all acquire `inner.write().await` before calling parse).

**Audit every read-lock handler** to ensure it does NOT call `parse_with_format_plugin`
or any `&mut` method. If any does, it must be changed to acquire the write lock.

**Specific handlers to audit**:
- `semantic.rs::semantic_tokens_full` — reads cache only ✓ (Phase 2)
- `completion.rs` — reads registry via `get()` ✓
- `hover.rs` — reads registry via `get()` ✓
- `navigation.rs` — reads registry via `get()` ✓
- `variables.rs` — reads registry via `get()` ✓
- `structure.rs` — reads registry via `get()` ✓
- `code_actions.rs` — reads registry via `get()` ✓

### 4.7 Remove parking_lot Dependency

**File**: `Cargo.toml` (workspace root)
**File**: `crates/formats/Cargo.toml`

Remove `parking_lot` from dependencies.

### 4.8 Validation

- [ ] `cargo check` passes — the borrow checker is now enforcing correctness
- [ ] `cargo test` passes
- [ ] Extension starts
- [ ] .tw file opens with highlighting
- [ ] Rapid file switching → no deadlock
- [ ] Large workspace indexing → no starvation (yield_now between files)
- [ ] Variable Flow panel works (reads from registry under read lock)
- [ ] Completion works (reads from registry under read lock)
- [ ] Hover works (reads from registry under read lock)

---

## Phase 5: Arena-Based VariableTree

### Goal

Replace the HashMap-based VariableTree with an arena-allocated tree for O(1) path
lookups and better memory locality. This is a must-have feature.

### Why This Phase After FormatPluginMut

The arena VariableTree changes the internal data structure of `SugarCubeRegistry`.
All access patterns (read and write) must be updated. Doing this after
`FormatPluginMut` means the borrow checker will catch any code that tries to
access the old API through `&self` when it should use `&mut self`.

### 5.1 Port Arena Types from ver_3

**File**: `crates/formats/src/sugarcube/registries/variable_tree.rs`

Port the following types from the current ver_3 code:
- `NodeId` (newtype for `u32`, with `NO_NODE` sentinel)
- `VarArena` (`Vec<VarArenaNode>`)
- `VarArenaNode` (first-child/next-sibling, `VarMeta` payload)
- `VarMeta` (name, access list, scope, inferred type, source locations)
- `NavIndex` (file_uri → passage_name → line mapping for go-to-def)
- `VarScope` (Persistent/Temporary)
- `InferredType` (Scalar/Object/Array/Unknown)
- `SourceLocation` (file_uri, passage_name, span, line)

### 5.2 Implement Arena-Based VariableTree

**Same file**: `variable_tree.rs`

**Key methods to implement** (API-compatible with the old HashMap-based tree):

**Read methods** (`&self`):
- `get_variable(&self, name: &str) -> Option<&VarEntry>` — O(1) via `path_index`
- `variable_names(&self) -> Vec<String>` — iterate `path_index`
- `all_variables(&self) -> Vec<(&str, &VarEntry)>` — iterate arena
- `compute_passage_positions(&self, ...) -> PassagePositionMap`

**Write methods** (`&mut self`):
- `record_var(&mut self, name, access, passage, file_uri, ...)` — insert or find node in arena
- `remove_file(&mut self, file_uri)` — walk arena, remove matching accesses, prune dead nodes
- `remove_passage(&mut self, passage_name)` — walk arena, remove matching accesses
- `clear(&mut self)` — reset arena and indices

**The `record_var` algorithm**:
1. Split `$player.hp.max` into segments: `["$player", "hp", "max"]`
2. Look up or create each segment in the arena (first-child/next-sibling traversal)
3. Record the `VarAccess` on the LEAF node
4. Propagate the access UP to all ancestor nodes (mark as `propagated: true`)

**The `remove_file` algorithm**:
1. Walk ALL nodes in the arena
2. For each node, remove `VarAccess` entries where `file_uri` matches
3. Prune nodes with zero accesses AND zero children
4. Rebuild `path_index` if any nodes were pruned

**Control logic for `remove_file`**: This is the most performance-sensitive
operation. The arena walk is O(N) where N = total variable nodes. For a large
workspace, this could be thousands of nodes. But since we already hold the write
lock and no other code can access the registry during this operation, there's no
contention concern. The `yield_now()` between files in indexing ensures other
tasks get a chance.

### 5.3 Migrate LegacyVariableTree Tests Before Deletion

**Before deleting `LegacyVariableTree`**, migrate all ~15 test functions that use
`LegacyVariableTree::legacy_new()` to use the arena-based `VariableTree::new()`.
These tests are in `variable_tree.rs` at lines ~2592-2795.

The test migration is straightforward — `VariableTree::new()` has the same public
API as `LegacyVariableTree::legacy_new()`. Replace the constructor and verify
each test still passes.

**After all tests are migrated and passing**, delete `LegacyVariableTree` and its
`legacy_new()` constructor. Do NOT keep both implementations — the coexistence is
a maintenance hazard.

### 5.4 Update All Consumers

**Files that reference the old VariableTree API**:
- `var_extract.rs` — functions like `extract_passage_variable_refs`,
  `build_shape_aware_property_map`, `build_state_variable_registry`
- `registry_populate.rs` — `populate_registries_from_ast`, `walk_script_js`,
  `walk_inline_js_snippets`
- `mod.rs` (SugarCubePlugin) — `build_variable_tree`, `variable_names`,
  `variable_properties`, etc.
- `token_builder.rs` — token emission from var ops

**Control logic**: The arena tree's read API (`&self`) must be identical to
the old tree's API from the consumers' perspective. Only the internal
implementation changes. If any consumer needs a method that doesn't exist on
the arena tree, add it — don't change the consumer.

### 5.5 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes (especially variable_tree unit tests)
- [ ] Extension starts
- [ ] .tw file with `$var` references → Variable Flow shows correct hierarchy
- [ ] `$player.hp.max` → tree shows player → hp → max
- [ ] `State.variables.ITEMS` → detected and shown in tree
- [ ] Remove a file → only that file's variable entries are removed
- [ ] Large workspace → no performance regression in variable lookup

---

## Phase 6: Unified 3-Phase Parse Pipeline + JsAnalysis

### Goal

Replace the dual-path variable extraction (SugarCube parser + oxc walker
independently) with a unified 3-phase pipeline:
1. Phase 1: Structural parse → `PassageAst` (with `js_analysis: None`)
2. Phase 2: JS annotation → fills `js_analysis` on each node
3. Phase 3: Registry population → single walk over enriched AST

### Why This Phase After Arena VariableTree

The unified pipeline changes how variables are recorded in the VariableTree.
If the arena tree is already in place, we can validate that the new pipeline
produces the same variable entries as the old dual-path approach.

### 6.1 Add JsAnalysis Struct

**File**: `crates/formats/src/sugarcube/ast.rs`

```rust
/// JS analysis results attached to an AST node after oxc parsing.
/// Produced by the js_annotate phase (Phase 2 of the unified pipeline).
///
/// ALL four fields are critical — they replace the separate scanning paths
/// that previously existed (walk_script_js, walk_inline_js_snippets, etc.).
#[derive(Debug, Clone, Default)]
pub struct JsAnalysis {
    /// Variable operations detected by oxc (reads, writes, compound writes, etc.)
    /// Replaces the old dual-path: SugarCube parser's var_refs + oxc walker's var_ops.
    pub var_ops: Vec<AnalyzedVarOp>,

    /// Macro.add("name", {...}) calls detected in script/inline JS.
    /// Replaces the old walk_script_js macro detection.
    pub macro_adds: Vec<MacroAddInfo>,

    /// Template.add("name", ...) calls detected in script/inline JS.
    /// Replaces the old walk_script_js template detection.
    pub template_adds: Vec<TemplateAddInfo>,

    /// Function declarations in [script] passages.
    /// Replaces the old walk_script_js function detection.
    pub function_defs: Vec<FunctionDefInfo>,
}

#[derive(Debug, Clone)]
pub struct AnalyzedVarOp {
    pub name: String,
    pub kind: VarAccessKind,
    pub span: Range<usize>,
    pub is_temporary: bool,
}
```

**NOTE**: `MacroAddInfo`, `TemplateAddInfo`, and `FunctionDefInfo` are already
defined in `crates/formats/src/types.rs` at f65d6e2. Verify these types have
the fields needed by `populate_registries_from_unified_ast` before proceeding.

**Modify `AstNode::Macro`**:
```rust
AstNode::Macro {
    name: String,
    args: String,
    children: Vec<AstNode>,
    set_assignment: Option<SetAssignment>,
    js_analysis: Option<JsAnalysis>,  // ← ADDED (initially None)
}
```

**Modify `AstNode::Expression`**:
```rust
AstNode::Expression {
    content: String,
    js_analysis: Option<JsAnalysis>,  // ← ADDED (initially None)
}
```

**Modify `PassageAst`**:
```rust
pub struct PassageAst {
    pub nodes: Vec<AstNode>,
    pub links: Vec<LinkInfo>,
    pub var_ops: Vec<VarOpInfo>,  // keep temporarily for backward compat
    pub mode: ParseMode,
    pub script_js_analysis: Option<JsAnalysis>,  // ← ADDED (initially None)
}
```

### 6.2 Create js_annotate.rs

**File**: `crates/formats/src/sugarcube/js/js_annotate.rs`

```rust
/// Annotate AST nodes with JS analysis results.
///
/// This is Phase 2 of the unified 3-phase parse pipeline.
/// It walks the AST, finds nodes containing JS (<<set>>, <<run>>, <<if>>,
/// <<script>> blocks), preprocesses the JS for oxc, parses it, and attaches
/// JsAnalysis to each node.
///
/// For script passages, the entire body is parsed as a JS module and the
/// result is stored in PassageAst::script_js_analysis.

pub fn annotate_js(
    ast: &mut PassageAst,
    mode: ParseMode,
    passage_name: &str,
    file_uri: &str,
    body_text: &str,
) {
    match mode {
        ParseMode::Script => annotate_script_passage(ast, passage_name, file_uri, body_text),
        ParseMode::Normal | ParseMode::Widget | ParseMode::Interface => {
            annotate_inline_js(&mut ast.nodes, passage_name, file_uri, body_text)
        }
        ParseMode::Stylesheet | ParseMode::Minimal => {
            // No JS to annotate in stylesheet/minimal passages
        }
    }
}
```

**`annotate_script_passage`**: Collect all text from AST nodes → preprocess
for oxc → parse as Module → walk AST → produce JsAnalysis → store in
`ast.script_js_analysis`.

**`annotate_inline_js`**: Walk AST nodes → for each Macro/Expression node
containing `$` or `_` in args → collect JS snippet → preprocess → parse as
Expression or Module → produce JsAnalysis → store in node's `js_analysis`.

### 6.3 Create populate_registries_from_unified_ast

**File**: `crates/formats/src/sugarcube/registries/registry_populate.rs`

```rust
/// Populate registries from a unified AST (Phase 3).
///
/// Walks the AST once, reading js_analysis from nodes that have it.
/// Falls back to var_ops for nodes without js_analysis (backward compat).
pub fn populate_registries_from_unified_ast(
    registry: &mut SugarCubeRegistry,
    ast: &PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
) {
    // 1. Script passage: populate from script_js_analysis
    if let Some(ref analysis) = ast.script_js_analysis {
        for var_op in &analysis.var_ops {
            registry.record_var(...);
        }
        for macro_add in &analysis.macro_adds {
            registry.custom_macros_mut().add(...);
        }
        // ... etc for templates, functions
    }

    // 2. Walk AST nodes, populate from js_analysis or var_ops
    for node in &ast.nodes {
        match node {
            AstNode::Macro { js_analysis: Some(analysis), set_assignment, .. } => {
                for var_op in &analysis.var_ops { registry.record_var(...); }
                // Apply SugarCube semantic overrides:
                // <<capture>> → Capture, <<unset>> → Unset
            }
            AstNode::Macro { js_analysis: None, var_refs, .. } => {
                // Fallback: use var_refs from SugarCube parser
                for var_ref in var_refs { registry.record_var(...); }
            }
            // ... similar for Expression, Text nodes
        }
    }
}
```

### 6.4 Update parse_pipeline.rs

**File**: `crates/formats/src/sugarcube/parse_pipeline.rs`

**In `parse_full`, replace the dual-path with unified 3-phase**:

**Before** (f65d6e2 + bug fixes):
```rust
// Phase 1: structural parse
let passage_ast = parser::parse_passage_body(body, body_offset, mode);

// OLD Phase 2 (dual-path): separate calls for registry population
if is_script_passage {
    walk_script_js(registry, ...);
} else {
    populate_registries_from_ast(registry, ...);
    walk_inline_js_snippets(registry, ...);  // second oxc walk
}
```

**After** (unified 3-phase):
```rust
// Phase 1: structural parse
let mut passage_ast = parser::parse_passage_body(body, body_offset, mode);

// Phase 2: JS annotation (NEW)
js_annotate::annotate_js(&mut passage_ast, mode, &cp.header.name, uri.as_ref(), &cp.body_text);

// Phase 3: unified registry population
registry_populate::populate_registries_from_unified_ast(registry, &passage_ast, &cp, uri.as_ref());
```

**Remove** the old `walk_inline_js_snippets()` call and `walk_script_js()` call.
They're replaced by Phase 2 + Phase 3.

### 6.5 Update token_builder.rs

**File**: `crates/formats/src/sugarcube/lsp/token_builder.rs`

**Change token emission to read from `js_analysis` instead of `var_refs`**:

```rust
// For Macro nodes with js_analysis:
if let Some(ref analysis) = node.js_analysis {
    for var_op in &analysis.var_ops {
        self.emit_var_op_token(var_op);
    }
} else {
    // Fallback: use var_refs for nodes without js_analysis
    for var_ref in &node.var_refs {
        self.emit_var_ref_token(var_ref);
    }
}
```

### 6.6 Remove Old `walk_inline_js_snippets` — But KEEP `scan_inline_vars`

**Remove** `walk_inline_js_snippets()` function from `registry_populate.rs`.
This function is replaced by the `js_annotate` phase, which handles inline JS
as part of the unified pipeline.

**DO NOT remove `scan_inline_vars()`** from `parser/variable_scan.rs`.
This function handles `$var` references in **prose text** (plain text between
macros), which is SugarCube template syntax that oxc cannot parse. For example:
```
You have $gold coins and $hp health.
```
These are detected by `scan_inline_vars()` and stored in `AstNode::Text { var_refs }`.
The unified pipeline correctly handles them in `populate_registries_from_unified_ast`
by converting `VarRef` → `AnalyzedVarOp` with `VarAccessKind::Read`.

**After all consumers are migrated**, you MAY remove `var_ops: Vec<VarOpInfo>` from
`PassageAst`. But `var_refs: Vec<VarRef>` on `AstNode::Text` must remain —
it's the ONLY source of prose variable detection.

**The transition is**:
- `AstNode::Macro` → reads from `js_analysis.var_ops` (oxc-detected)
- `AstNode::Expression` → reads from `js_analysis.var_ops` (oxc-detected)
- `AstNode::Text` → reads from `var_refs` (SugarCube scanner-detected, NOT oxc)
- `PassageAst.var_ops` → can be removed once all code reads from `js_analysis`

### 6.7 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] Extension starts
- [ ] `$var` references are detected (SugarCube parser path)
- [ ] `State.variables.x` references are detected (oxc path via js_analysis)
- [ ] `<<set $hp to 100>>` → $hp recorded as Write
- [ ] `<<set $hp += 10>>` → $hp recorded as CompoundWrite
- [ ] `<<run _items = State.variables.ITEMS>>` → _items as Write, $ITEMS as Read
- [ ] Semantic tokens highlight variables correctly
- [ ] No duplicate variable entries (single walk instead of dual-path)
- [ ] Script passages → variables detected via script_js_analysis
- [ ] Widget passages → variables detected via node js_analysis

---

## Phase 7: Per-Segment Span Tracking

### Goal

Add per-token span tracking for precision go-to-definition on dotted property
paths like `$foo.bar.baz`.

### 7.1 Add Span Fields to AnalyzedVarOp

**File**: `crates/formats/src/sugarcube/ast.rs`

```rust
pub struct AnalyzedVarOp {
    pub name: String,
    pub kind: VarAccessKind,
    pub span: Range<usize>,
    pub is_temporary: bool,
    pub segment_spans: Vec<Range<usize>>,       // ← ADDED
    pub construct_span: Option<Range<usize>>,    // ← ADDED
}
```

`segment_spans` provides per-token highlighting: `[$foo, .bar, .baz]` each get
their own span. `construct_span` covers the entire `{...}` for object literal
writes like `<<set $foo = {bar: 1}>>`.

### 7.2 Compute Segment Spans in js_annotate

**File**: `crates/formats/src/sugarcube/js/js_annotate.rs`

When building `AnalyzedVarOp` from oxc AST nodes, compute per-segment spans:
- For `$foo.bar.baz`: split on `.`, compute span for each segment
- For `State.variables.foo.bar`: map back from substituted names, compute spans

### 7.3 Pass Segment Spans Through to VariableTree

**File**: `crates/formats/src/sugarcube/registries/variable_tree.rs`

Update `record_var()` signature:
```rust
pub fn record_var(
    &mut self,
    name: &str,
    access: VarAccess,
    segment_spans: &[Range<usize>],      // ← ADDED
    construct_span: Option<Range<usize>>, // ← ADDED
)
```

Store `segment_spans` and `construct_span` in `VarAccess` (in the arena's `VarMeta`).

### 7.4 Emit Per-Segment Tokens

**File**: `crates/formats/src/sugarcube/lsp/token_builder.rs`

```rust
fn emit_var_op_tokens(&mut self, var_op: &AnalyzedVarOp) {
    if var_op.segment_spans.is_empty() {
        // Fallback: single token for the entire variable
        self.emit_token(var_op.span.clone(), SemanticTokenType::Variable, ...);
    } else {
        // Per-segment tokens: $foo .bar .baz each get their own token
        for (i, span) in var_op.segment_spans.iter().enumerate() {
            let token_type = if i == 0 {
                SemanticTokenType::Variable  // $foo
            } else {
                SemanticTokenType::Property  // .bar, .baz
            };
            self.emit_token(span.clone(), token_type, ...);
        }
    }
}
```

### 7.5 Validation

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] `$foo.bar.baz` → each segment highlighted individually
- [ ] `$foo` → variable token
- [ ] `.bar`, `.baz` → property tokens
- [ ] Go-to-definition on `$foo.bar.baz` navigates to the write location
- [ ] `<<set $foo = {bar: 1}>>` → construct span covers entire `{bar: 1}`

---

## Phase 8: Optional Improvements

### 8A: StoryData JSON Tokenization

- Add `build_json_body_tokens()` to `token_builder.rs`
- For `ParseMode::Minimal` (StoryData), tokenize JSON property names, strings, numbers
- Make `ParseMode::Minimal` produce a `Text` node with JSON body content

### 8B: Pipeline Logging

- Create `pipeline_log.rs` with structured trace events
- Add logging at every handler entry/exit point
- Log to stderr (visible in VS Code output panel)

### 8C: Workspace::apply_document_update()

- Centralize graph_surgery + metadata + upstream-edge code
- Replace scattered logic in did_open/did_change with single call

### 8D: ProfileViewProvider Polling

- Replace single-shot `refresh()` with retry-based polling
- Add `dispose()` for timer cleanup

---

## Phase Dependency Graph

```
Phase 1 (baseline + fixes)
    │
    ├── Phase 2 (token caching) ← infrastructure for all subsequent phases
    │       │
    │       └── Phase 3 (cascade protection) ← depends on token cache
    │               │
    │               └── Phase 4 (FormatPluginMut) ← safe now that cache + cascade exist
    │                       │
    │                       ├── Phase 5 (arena VariableTree) ← needs &mut self
    │                       │       │
    │                       │       └── Phase 6 (unified pipeline) ← needs arena tree
    │                       │               │
    │                       │               └── Phase 7 (segment spans) ← needs unified pipeline
    │                       │
    │                       └── Phase 8 (optional improvements)
```

## Critical Invariants (Must Hold After EVERY Phase)

| # | Invariant | Verified By |
|---|-----------|-------------|
| 1 | `cargo check` passes | Compiler |
| 2 | `cargo test` passes | Test runner |
| 3 | Extension starts without error | Manual / VS Code dev console |
| 4 | .tw file shows semantic highlighting | Manual |
| 5 | No deadlock during indexing + file switching | Manual (rapid switching) |
| 6 | No permanent token loss after format switch | Manual (open project with StoryData) |
| 7 | Variable Flow panel shows correct entries | Manual |
| 8 | Edit → save → correct re-parse | Manual |
| 9 | `remove_file` is called exactly once per parse (in `parse_full`) | Code review |
| 10 | `remove_passage_from_registries` does NOT call `remove_file` | Code review |
| 11 | No read-lock handler calls `parse_with_format_plugin` (after Phase 4) | Code review + borrow checker |

## Rollback Strategy

After each phase, create a git tag:
```
git tag phase-1-baseline
git tag phase-2-token-cache
git tag phase-3-cascade-protection
...
```

If a phase introduces breakage, `git reset --hard phase-N-1` to the last known-good
state and re-attempt the phase with the fix.

---

## Interrogation Report

> This section documents the 12 discrepancies found during aggressive interrogation
> of the plan against the actual codebase at both f65d6e2 and ver_3 HEAD. Each
> finding is classified by severity and shows what was corrected.

### Methodology

Two parallel interrogation agents were launched:
1. **Agent A**: Verified Phase 1-3 specifications against the actual f65d6e2 code
2. **Agent B**: Verified Phase 4-7 specifications against the actual ver_3 HEAD code

Each agent read the actual source files, compared them against the plan claims,
and reported discrepancies.

---

### Finding 1: FATAL - parse_single() has 4 parameters, not 3

**Phase affected**: Phase 1.3

**Plan claimed**: `parse_single()` takes 3 parameters: `plugin`, `passage_name`, `passage_text`

**Actual state at f65d6e2**: Takes 4 parameters: `plugin`, `passage_name`, `passage_tags`, `passage_text`
- the `passage_tags: &[String]` parameter was completely omitted from the plan.

**Impact**: If implemented as written, the function signature would be wrong and
all callers would need different arguments than planned.

**Correction applied**: Updated Phase 1.3 to show the correct 4 to 5 parameter change
(adding `file_uri` after `passage_text`), and added a note that `passage_tags` was
missed in the original plan.

---

### Finding 2: FATAL - FormatPlugin::parse_passage() trait method must also be updated

**Phase affected**: Phase 1.3

**Plan claimed**: Only `parse_single()` in `parse_pipeline.rs` needs the `file_uri` parameter.

**Actual state at f65d6e2**: The `FormatPlugin` trait in `plugin.rs` defines
`fn parse_passage(&self, passage_name, passage_tags, passage_text) -> Option<Passage>`.
Since `parse_single()` is called through this trait method, the TRAIT must also be
updated, AND every format plugin implementation (SugarCube, Harlowe, Chapbook, Snowman,
TwineCore) must be updated.

**Impact**: Without updating the trait, the compiler would reject the new parameter
on the concrete implementation.

**Correction applied**: Added explicit instructions to update the `FormatPlugin::parse_passage()`
trait method signature and listed all 5 format plugin implementations that must be updated.

---

### Finding 3: FATAL - ServerState.inner is RwLock<...>, NOT Arc<RwLock<...>>

**Phase affected**: Phase 3.1

**Plan claimed**: Phase 3 adds `Arc` wrapping around `RwLock<ServerStateInner>`.

**Actual state at f65d6e2**: `ServerState.inner` is `tokio::sync::RwLock<ServerStateInner>`,
NOT wrapped in `Arc`. Since `tokio::spawn` requires `'static + Send` futures and
`RwLock` is not `Clone`, we cannot clone `self.inner` into a spawned task without
wrapping it in `Arc` first.

**Impact**: The `initialized` handler `tokio::spawn` would fail to compile because
`self.inner` cannot be cloned.

**Correction applied**: Added explicit note that Phase 3.1 must change `inner` from
`RwLock<ServerStateInner>` to `Arc<RwLock<ServerStateInner>>`, and that all handler
methods that use `self.inner.read().await` and `self.inner.write().await` still work
because `Arc` derefs to the inner type.

---

### Finding 4: HIGH - "Double remove_file" claim was FALSE

**Phase affected**: Phase 1.4

**Plan originally claimed**: Both callers AND `parse_full()` call `registry.remove_file()`,
creating a double-removal that needs fixing.

**Actual state at f65d6e2**: After thorough code audit of ALL call sites:
- `parse_full()` calls `registry.remove_file(uri.as_ref())` only
- `did_open` does NOT call `remove_file_from_registries` before parse
- `did_change` does NOT call `remove_file_from_registries` before parse
- `indexing.rs` Pass 2 does NOT call `remove_file_from_registries`
- `did_change_watched_files` DELETED case calls `remove_file_from_registries` - but
  this is AFTER removing the document (cleanup), not before a parse

**Impact**: If we had "fixed" a non-existent double-removal by moving `remove_file`
out of `parse_full()`, we would have SPREAD responsibility to multiple call sites,
increasing the risk of missing one.

**Correction applied**: Changed Phase 1.4 from "fix double remove" to "verify no change
needed" with a documented control logic for future maintainers.

---

### Finding 5: HIGH - convert_semantic_tokens already exists, must not be recreated

**Phase affected**: Phase 2.3

**Plan implied**: The new cache-first `semantic_tokens_full` would need a function
to convert format-plugin `SemanticToken` to LSP `SemanticToken`.

**Actual state at f65d6e2**: `convert_semantic_tokens()` already exists in
`semantic.rs` (lines 66-120), with byte-to-line/char conversion and important
clamping/safety logic.

**Impact**: Recreating this function would lose the safety clamping logic and
create maintenance burden of two copies.

**Correction applied**: Added explicit "DO NOT recreate this function" note in Phase 2.3
with reference to the existing function location.

---

### Finding 6: HIGH - semantic_tokens_full re-parses under read lock, causing hidden writes

**Phase affected**: Phase 2.3

**Plan described**: The old behavior "re-parses on every request."

**Actual concurrency issue**: At f65d6e2, `semantic_tokens_full` acquires a READ lock
on `ServerStateInner`, but `parse_full()` internally calls `registry.remove_file()`
and `populate_registries_from_ast()`, which WRITE to the `SugarCubeRegistry`
sub-registries via `parking_lot::RwLock`. This means a "read" operation on the server
state is actually triggering hidden writes through interior mutability.

**Impact**: This is architecturally unsound. The token cache fix eliminates this
concurrency correctness problem entirely, which is a benefit beyond just performance.

**Correction applied**: Added "IMPORTANT CONCURRENCY NOTE" to Phase 2.3 documenting
this hidden-write issue and how the cache fix resolves it.

---

### Finding 7: HIGH - scan_inline_vars must NOT be removed

**Phase affected**: Phase 6.6

**Plan implied**: The unified pipeline would replace inline variable scanning with
oxc-based `js_annotate`.

**Actual capability gap**: `scan_inline_vars` handles prose `$var` detection that
oxc CANNOT detect because oxc only processes `<script>` blocks. Prose variable
references like `You have $player.hp hit points` are NOT inside `<script>` tags
and must be detected by the regex-based scanner.

**Impact**: Removing `scan_inline_vars` would cause ALL prose variable references
to be invisible to the Variable Flow panel and semantic highlighting.

**Correction applied**: Added explicit "KEEP scan_inline_vars" warning in Phase 6.6
with explanation of the prose detection gap.

---

### Finding 8: HIGH - LegacyVariableTree has 15+ tests that need migration before deletion

**Phase affected**: Phase 5.3

**Plan originally said**: Delete `LegacyVariableTree` after arena tree is implemented.

**Actual state at ver_3**: There are approximately 15 test functions that use
`LegacyVariableTree::legacy_new()`. Deleting the type without migrating these tests
would leave the test suite with compilation errors.

**Impact**: Without migrating tests, `cargo test` would fail, violating the Phase
Execution Contract.

**Correction applied**: Changed Phase 5.3 from "delete" to "migrate tests before deletion"
with explicit instruction to update all test functions to use arena-based `VariableTree::new()`.

---

### Finding 9: HIGH - FormatPlugin::parse(&self) was REMOVED in ver_3, not just moved

**Phase affected**: Phase 4.5

**Plan implied**: The `parse(&self)` method would be "moved" to `FormatPluginMut::parse_mut(&mut self)`.

**Actual state at ver_3**: The `parse(&self)` method was completely REMOVED from the
`FormatPlugin` trait. ALL parsing goes through `FormatPluginMut::parse_mut()`. There
is no read-only parse method.

**Impact**: If we kept `parse(&self)` on `FormatPlugin`, we would need to decide what it
does - but the whole point of FormatPluginMut is that parsing requires exclusive access.
Keeping a read-only parse would undermine the architectural change.

**Correction applied**: Added explicit "REMOVE `fn parse(&self, ...)` from the
FormatPlugin trait" instruction in Phase 4.5.

---

### Finding 10: MEDIUM - FormatRegistry should NOT have a separate FormatRegistryMut trait

**Phase affected**: Phase 4.5

**Plan originally considered**: A separate `FormatRegistryMut` trait for mutable access.

**Actual design at ver_3**: `FormatPluginMut: FormatPlugin` means every `FormatPluginMut`
is also a `FormatPlugin`. So `Box<dyn FormatPluginMut>` can produce both `&dyn FormatPlugin`
(via `as_ref()`) and `&mut dyn FormatPluginMut` (via `as_mut()`). A single `FormatRegistry`
struct with both `get()` and `get_mut()` methods is sufficient.

**Correction applied**: Simplified Phase 4.5 to use a single `FormatRegistry` with
both access patterns, removing the `FormatRegistryMut` concept.

---

### Finding 11: MEDIUM - JsAnalysis has 4 fields, not just var_ops

**Phase affected**: Phase 6.1

**Plan implied**: `JsAnalysis` primarily contains `var_ops`.

**Actual state at ver_3**: `JsAnalysis` contains 4 fields:
1. `var_ops: Vec<VarOp>` - variable reads/writes
2. `macro_calls: Vec<MacroCall>` - macro invocations
3. `function_calls: Vec<FunctionCall>` - function invocations
4. `templates: Vec<TemplateUsage>` - template usage

**Impact**: The plan must account for all 4 categories being populated in the
js_annotate phase, not just variable operations.

**Correction applied**: Expanded Phase 6.1 to document all 4 JsAnalysis fields
with their purpose.

---

### Finding 12: MEDIUM - Arena VariableTree is 1600+ lines; js_annotate.rs is 758 lines

**Phase affected**: Phase 5, Phase 6

**Plan implied**: These are moderate-sized implementations.

**Actual size at ver_3**: The arena-based `VariableTree` in `variable_tree.rs` is
approximately 1600 lines of Rust. The `js_annotate.rs` file is approximately 758 lines.
These are substantial implementations that need careful porting, not trivial additions.

**Impact**: Underestimating complexity could lead to rushing and introducing bugs.

**Correction applied**: Added size estimates to the affected phases as a heads-up
for implementation planning.

---

### Summary of Corrections

| # | Severity | Finding | Phase | Correction |
|---|----------|---------|-------|------------|
| 1 | FATAL | parse_single has 4 params not 3 | 1.3 | Fixed signature, added passage_tags |
| 2 | FATAL | FormatPlugin::parse_passage trait must update | 1.3 | Added trait + all impl updates |
| 3 | FATAL | ServerState.inner not wrapped in Arc | 3.1 | Added Arc wrapping requirement |
| 4 | HIGH | Double remove_file claim was false | 1.4 | Changed to verify no change needed |
| 5 | HIGH | convert_semantic_tokens already exists | 2.3 | Added do not recreate warning |
| 6 | HIGH | Hidden writes under read lock | 2.3 | Documented concurrency issue |
| 7 | HIGH | scan_inline_vars must be kept | 6.6 | Added explicit keep warning |
| 8 | HIGH | 15+ tests need migration | 5.3 | Changed to migrate before delete |
| 9 | HIGH | parse(&self) removed, not moved | 4.5 | Added explicit removal instruction |
| 10 | MEDIUM | No separate FormatRegistryMut | 4.5 | Simplified to single FormatRegistry |
| 11 | MEDIUM | JsAnalysis has 4 fields | 6.1 | Expanded JsAnalysis documentation |
| 12 | MEDIUM | Arena tree 1600+ lines | 5,6 | Added size estimates |
