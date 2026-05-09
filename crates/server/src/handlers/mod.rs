//! LSP `LanguageServer` trait implementation and handler submodules.
//!
//! This module contains the core request/notification handlers that wire
//! knot-core and knot-formats into the Language Server Protocol via
//! tower-lsp. All parsing is delegated to format plugins from `knot-formats`.

// Clippy pedantic suppressions for this module:
#![allow(clippy::manual_strip)]
#![allow(clippy::match_like_matches_macro)]

pub mod call_hierarchy;
pub mod code_actions;
pub mod completion;
pub mod editing;
pub mod helpers;
pub mod hover;
pub mod knot_ext;
pub mod lifecycle;
pub mod macros;
pub mod navigation;
pub mod semantic;
pub mod structure;
pub mod sync;

use crate::state::ServerState;
use lsp_types::*;
use tower_lsp::LanguageServer;

// ---------------------------------------------------------------------------
// LanguageServer trait implementation — thin delegators
// ---------------------------------------------------------------------------

#[tower_lsp::async_trait]
impl LanguageServer for ServerState {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
        lifecycle::initialize(self, params).await
    }

    async fn initialized(&self, params: InitializedParams) {
        lifecycle::initialized(self, params).await
    }

    async fn shutdown(&self) -> Result<(), tower_lsp::jsonrpc::Error> {
        lifecycle::shutdown(self).await
    }

    // -----------------------------------------------------------------------
    // Document synchronization
    // -----------------------------------------------------------------------

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        sync::did_open(self, params).await
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        sync::did_change(self, params).await
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        sync::did_close(self, params).await
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        sync::did_save(self, params).await
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        sync::did_change_configuration(self, params).await
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        sync::did_change_watched_files(self, params).await
    }

    // -----------------------------------------------------------------------
    // Language features
    // -----------------------------------------------------------------------

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
        completion::completion(self, params).await
    }

    async fn completion_resolve(
        &self,
        params: CompletionItem,
    ) -> Result<CompletionItem, tower_lsp::jsonrpc::Error> {
        completion::completion_resolve(self, params).await
    }

    async fn hover(
        &self,
        params: HoverParams,
    ) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
        hover::hover(self, params).await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        navigation::goto_definition(self, params).await
    }

    async fn goto_declaration(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        navigation::goto_declaration(self, params).await
    }

    async fn goto_implementation(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        navigation::goto_implementation(self, params).await
    }

    async fn goto_type_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        navigation::goto_type_definition(self, params).await
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>, tower_lsp::jsonrpc::Error> {
        navigation::references(self, params).await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>, tower_lsp::jsonrpc::Error> {
        semantic::document_symbol(self, params).await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>, tower_lsp::jsonrpc::Error> {
        semantic::semantic_tokens_full(self, params).await
    }

    async fn code_lens(
        &self,
        params: CodeLensParams,
    ) -> Result<Option<Vec<CodeLens>>, tower_lsp::jsonrpc::Error> {
        semantic::code_lens(self, params).await
    }

    async fn inlay_hint(
        &self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>, tower_lsp::jsonrpc::Error> {
        semantic::inlay_hint(self, params).await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>, tower_lsp::jsonrpc::Error> {
        semantic::symbol(self, params).await
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>, tower_lsp::jsonrpc::Error> {
        structure::signature_help(self, params).await
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>, tower_lsp::jsonrpc::Error> {
        code_actions::code_action(self, params).await
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        editing::formatting(self, params).await
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        editing::range_formatting(self, params).await
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        editing::on_type_formatting(self, params).await
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>, tower_lsp::jsonrpc::Error> {
        editing::linked_editing_range(self, params).await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>, tower_lsp::jsonrpc::Error> {
        editing::prepare_rename(self, params).await
    }

    async fn rename(
        &self,
        params: RenameParams,
    ) -> Result<Option<WorkspaceEdit>, tower_lsp::jsonrpc::Error> {
        editing::rename(self, params).await
    }

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>, tower_lsp::jsonrpc::Error> {
        structure::folding_range(self, params).await
    }

    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> Result<Option<Vec<DocumentLink>>, tower_lsp::jsonrpc::Error> {
        structure::document_link(self, params).await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>, tower_lsp::jsonrpc::Error> {
        structure::selection_range(self, params).await
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>, tower_lsp::jsonrpc::Error> {
        call_hierarchy::prepare_call_hierarchy(self, params).await
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>, tower_lsp::jsonrpc::Error> {
        call_hierarchy::incoming_calls(self, params).await
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>, tower_lsp::jsonrpc::Error> {
        call_hierarchy::outgoing_calls(self, params).await
    }

    // NOTE: The pull-diagnostic handler (`textDocument/diagnostic`) is intentionally
    // removed. The server uses the push model (`publish_diagnostics`) exclusively.
    // Running both models simultaneously causes VS Code to display every diagnostic
    // twice, which makes errors and warnings appear duplicated in hover.
}
