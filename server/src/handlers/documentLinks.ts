import {
  type Connection,
  DocumentLink,
  Range,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { normalizeUri, getFileText, offsetToPosition } from '../serverUtils';
import { passageNameFromExpr } from '../passageArgs';
import type { MarkupNode } from '../ast';
import { walkMarkup } from '../visitors';

export function registerDocumentLinkHandler(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  connection.onDocumentLinks(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];
    
    const normUri = normalizeUri(doc.uri);
    const cached = workspace.getParsedFile(normUri);
    const ast = cached?.ast;
    if (!ast) return [];
    
    const links: DocumentLink[] = [];
    const adapter = workspace.getActiveAdapter();
    const passageArgMacros = adapter.getPassageArgMacros();
    const fileText = doc.getText();
    
    // Walk AST finding [[link]] targets and passage-arg macro references
    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      collectLinks(passage.body, doc, workspace, links, adapter, passageArgMacros, fileText);
    }
    
    return links;
  });
}

function collectLinks(
  nodes: MarkupNode[],
  doc: TextDocument,
  workspace: WorkspaceIndex,
  links: DocumentLink[],
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
  passageArgMacros: ReadonlySet<string>,
  fileText: string,
): void {
  walkMarkup(nodes, {
    onLink(node) {
      // [[Target]] links
      const def = workspace.getPassageDefinition(node.target);
      if (def) {
        links.push(DocumentLink.create(
          Range.create(
            doc.positionAt(node.targetRange.start),
            doc.positionAt(node.targetRange.end),
          ),
          def.uri,
        ));
      }
    },
    onMacro(node) {
      // Passage arg macros like <<goto "Target">>, <<link "label" "Target">>
      if (passageArgMacros.has(node.name) && node.args.length > 0) {
        const idx = adapter.getPassageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg) {
          const targetName = passageNameFromExpr(arg);
          if (targetName) {
            const def = workspace.getPassageDefinition(targetName);
            if (def) {
              // Link the string content (inside quotes) to the passage definition
              links.push(DocumentLink.create(
                Range.create(
                  doc.positionAt(arg.range.start + 1),
                  doc.positionAt(arg.range.end - 1),
                ),
                def.uri,
              ));
            }
          }
        }
      }
    },
  });
}
