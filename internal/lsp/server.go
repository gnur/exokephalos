package lsp

import (
	"context"
	"io"
	"os"
	"sync"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/modern-dev/go-lsp/protocol"
	"go.lsp.dev/jsonrpc2"
)

type stdioReadWriteCloser struct {
	reader io.Reader
	writer io.Writer
}

func (s *stdioReadWriteCloser) Read(p []byte) (n int, err error) {
	return s.reader.Read(p)
}

func (s *stdioReadWriteCloser) Write(p []byte) (n int, err error) {
	return s.writer.Write(p)
}

func (s *stdioReadWriteCloser) Close() error {
	return nil
}

type Server struct {
	client    protocol.Client
	cache     *cache.Cache
	documents map[protocol.DocumentURI]string
	mu        sync.RWMutex
}

func NewServer(c *cache.Cache) *Server {
	return &Server{
		cache:     c,
		documents: make(map[protocol.DocumentURI]string),
	}
}

func (s *Server) Initialize(ctx context.Context, params *protocol.InitializeParams) (*protocol.InitializeResult, error) {
	openClose := true
	includeText := true
	changeKind := protocol.TextDocumentSyncKindFull
	version := "1.0.0"
	resolveSupport := true
	return &protocol.InitializeResult{
		Capabilities: protocol.ServerCapabilities{
			TextDocumentSync: &protocol.TextDocumentSyncOptions{
				OpenClose: &openClose,
				Change:    &changeKind,
				Save:      &protocol.SaveOptions{IncludeText: &includeText},
			},
			CompletionProvider: &protocol.CompletionOptions{
				TriggerCharacters: []string{"[", ":", "-"},
			},
			HoverProvider:      true,
			DefinitionProvider: true,
			CodeActionProvider: &protocol.CodeActionOptions{
				ResolveProvider: &resolveSupport,
			},
			InlayHintProvider: true,
		},
		ServerInfo: &protocol.ServerInfo{
			Name:    "exokephalos-lsp",
			Version: &version,
		},
	}, nil
}

func (s *Server) Initialized(ctx context.Context, params *protocol.InitializedParams) error {
	return nil
}

func (s *Server) Shutdown(ctx context.Context) (any, error) {
	return nil, nil
}

func (s *Server) Exit(ctx context.Context) error {
	return nil
}

func (s *Server) DidOpen(ctx context.Context, params *protocol.DidOpenTextDocumentParams) error {
	s.mu.Lock()
	s.documents[params.TextDocument.URI] = params.TextDocument.Text
	s.mu.Unlock()
	return nil
}

func (s *Server) DidChange(ctx context.Context, params *protocol.DidChangeTextDocumentParams) error {
	if len(params.ContentChanges) > 0 {
		if change, ok := params.ContentChanges[len(params.ContentChanges)-1].(map[string]any); ok {
			if text, ok := change["text"].(string); ok {
				s.mu.Lock()
				s.documents[params.TextDocument.URI] = text
				s.mu.Unlock()
			}
		}
	}
	return nil
}

func (s *Server) DidClose(ctx context.Context, params *protocol.DidCloseTextDocumentParams) error {
	s.mu.Lock()
	delete(s.documents, params.TextDocument.URI)
	s.mu.Unlock()
	return nil
}

func (s *Server) DidSave(ctx context.Context, params *protocol.DidSaveTextDocumentParams) error {
	if params.Text != nil {
		s.mu.Lock()
		s.documents[params.TextDocument.URI] = *params.Text
		s.mu.Unlock()
	}
	return nil
}

func (s *Server) Completion(ctx context.Context, params *protocol.CompletionParams) (any, error) {
	s.mu.RLock()
	text, ok := s.documents[params.TextDocument.URI]
	s.mu.RUnlock()
	if !ok {
		return nil, nil
	}

	compCtx, prefix := DetectCompletionContext(text, int(params.Position.Line), int(params.Position.Character))
	if compCtx == CompletionContextNone {
		return nil, nil
	}

	items, err := GetCompletions(ctx, s.cache, compCtx, prefix)
	if err != nil {
		return nil, err
	}

	return &protocol.CompletionList{
		IsIncomplete: false,
		Items:        items,
	}, nil
}

func (s *Server) Hover(ctx context.Context, params *protocol.HoverParams) (*protocol.Hover, error) {
	s.mu.RLock()
	text, ok := s.documents[params.TextDocument.URI]
	s.mu.RUnlock()
	if !ok {
		return nil, nil
	}

	return GetHover(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) Definition(ctx context.Context, params *protocol.DefinitionParams) (any, error) {
	s.mu.RLock()
	text, ok := s.documents[params.TextDocument.URI]
	s.mu.RUnlock()
	if !ok {
		return nil, nil
	}

	return GetDefinition(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) CancelRequest(ctx context.Context, params *protocol.CancelParams) error {
	return nil
}

func (s *Server) Progress(ctx context.Context, params *protocol.ProgressParams) error {
	return nil
}

func (s *Server) SetTrace(ctx context.Context, params *protocol.SetTraceParams) error {
	return nil
}

func (s *Server) IncomingCalls(ctx context.Context, params *protocol.CallHierarchyIncomingCallsParams) ([]protocol.CallHierarchyIncomingCall, error) {
	return nil, nil
}

func (s *Server) OutgoingCalls(ctx context.Context, params *protocol.CallHierarchyOutgoingCallsParams) ([]protocol.CallHierarchyOutgoingCall, error) {
	return nil, nil
}

func (s *Server) CodeActionResolve(ctx context.Context, params *protocol.CodeAction) (*protocol.CodeAction, error) {
	return ResolveCodeAction(ctx, params)
}

func (s *Server) CodeLensResolve(ctx context.Context, params *protocol.CodeLens) (*protocol.CodeLens, error) {
	return nil, nil
}

func (s *Server) CompletionResolve(ctx context.Context, params *protocol.CompletionItem) (*protocol.CompletionItem, error) {
	return nil, nil
}

func (s *Server) DocumentLinkResolve(ctx context.Context, params *protocol.DocumentLink) (*protocol.DocumentLink, error) {
	return nil, nil
}

func (s *Server) InlayHintResolve(ctx context.Context, params *protocol.InlayHint) (*protocol.InlayHint, error) {
	return nil, nil
}

func (s *Server) NotebookDocumentDidChange(ctx context.Context, params *protocol.DidChangeNotebookDocumentParams) error {
	return nil
}

func (s *Server) NotebookDocumentDidClose(ctx context.Context, params *protocol.DidCloseNotebookDocumentParams) error {
	return nil
}

func (s *Server) NotebookDocumentDidOpen(ctx context.Context, params *protocol.DidOpenNotebookDocumentParams) error {
	return nil
}

func (s *Server) NotebookDocumentDidSave(ctx context.Context, params *protocol.DidSaveNotebookDocumentParams) error {
	return nil
}

func (s *Server) CodeAction(ctx context.Context, params *protocol.CodeActionParams) ([]any, error) {
	s.mu.RLock()
	text, ok := s.documents[params.TextDocument.URI]
	s.mu.RUnlock()
	if !ok {
		return nil, nil
	}

	actions, err := GetCodeActions(ctx, text, int(params.Range.Start.Line), params.TextDocument.URI)
	if err != nil {
		return nil, err
	}

	result := make([]any, len(actions))
	for i := range actions {
		result[i] = actions[i]
	}
	return result, nil
}

func (s *Server) CodeLens(ctx context.Context, params *protocol.CodeLensParams) ([]protocol.CodeLens, error) {
	return nil, nil
}

func (s *Server) ColorPresentation(ctx context.Context, params *protocol.ColorPresentationParams) ([]protocol.ColorPresentation, error) {
	return nil, nil
}

func (s *Server) Declaration(ctx context.Context, params *protocol.DeclarationParams) (any, error) {
	return nil, nil
}

func (s *Server) Diagnostic(ctx context.Context, params *protocol.DocumentDiagnosticParams) (protocol.DocumentDiagnosticReport, error) {
	return nil, nil
}

func (s *Server) DocumentColor(ctx context.Context, params *protocol.DocumentColorParams) ([]protocol.ColorInformation, error) {
	return nil, nil
}

func (s *Server) DocumentHighlight(ctx context.Context, params *protocol.DocumentHighlightParams) ([]protocol.DocumentHighlight, error) {
	return nil, nil
}

func (s *Server) DocumentLink(ctx context.Context, params *protocol.DocumentLinkParams) ([]protocol.DocumentLink, error) {
	return nil, nil
}

func (s *Server) DocumentSymbol(ctx context.Context, params *protocol.DocumentSymbolParams) (any, error) {
	return nil, nil
}

func (s *Server) FoldingRanges(ctx context.Context, params *protocol.FoldingRangeParams) ([]protocol.FoldingRange, error) {
	return nil, nil
}

func (s *Server) Formatting(ctx context.Context, params *protocol.DocumentFormattingParams) ([]protocol.TextEdit, error) {
	return nil, nil
}

func (s *Server) Implementation(ctx context.Context, params *protocol.ImplementationParams) (any, error) {
	return nil, nil
}

func (s *Server) InlayHint(ctx context.Context, params *protocol.InlayHintParams) ([]protocol.InlayHint, error) {
	s.mu.RLock()
	text, ok := s.documents[params.TextDocument.URI]
	s.mu.RUnlock()
	if !ok {
		return nil, nil
	}

	return GetInlayHints(ctx, s.cache, text, int(params.Range.Start.Line), int(params.Range.End.Line))
}

func (s *Server) InlineValue(ctx context.Context, params *protocol.InlineValueParams) ([]protocol.InlineValue, error) {
	return nil, nil
}

func (s *Server) LinkedEditingRange(ctx context.Context, params *protocol.LinkedEditingRangeParams) (*protocol.LinkedEditingRanges, error) {
	return nil, nil
}

func (s *Server) Moniker(ctx context.Context, params *protocol.MonikerParams) ([]protocol.Moniker, error) {
	return nil, nil
}

func (s *Server) OnTypeFormatting(ctx context.Context, params *protocol.DocumentOnTypeFormattingParams) ([]protocol.TextEdit, error) {
	return nil, nil
}

func (s *Server) PrepareCallHierarchy(ctx context.Context, params *protocol.CallHierarchyPrepareParams) ([]protocol.CallHierarchyItem, error) {
	return nil, nil
}

func (s *Server) PrepareRename(ctx context.Context, params *protocol.PrepareRenameParams) (*protocol.PrepareRenameResult, error) {
	return nil, nil
}

func (s *Server) PrepareTypeHierarchy(ctx context.Context, params *protocol.TypeHierarchyPrepareParams) ([]protocol.TypeHierarchyItem, error) {
	return nil, nil
}

func (s *Server) RangeFormatting(ctx context.Context, params *protocol.DocumentRangeFormattingParams) ([]protocol.TextEdit, error) {
	return nil, nil
}

func (s *Server) References(ctx context.Context, params *protocol.ReferenceParams) ([]protocol.Location, error) {
	return nil, nil
}

func (s *Server) Rename(ctx context.Context, params *protocol.RenameParams) (*protocol.WorkspaceEdit, error) {
	return nil, nil
}

func (s *Server) SelectionRange(ctx context.Context, params *protocol.SelectionRangeParams) ([]protocol.SelectionRange, error) {
	return nil, nil
}

func (s *Server) SemanticTokensFull(ctx context.Context, params *protocol.SemanticTokensParams) (*protocol.SemanticTokens, error) {
	return nil, nil
}

func (s *Server) SemanticTokensFullDelta(ctx context.Context, params *protocol.SemanticTokensDeltaParams) (any, error) {
	return nil, nil
}

func (s *Server) SemanticTokensRange(ctx context.Context, params *protocol.SemanticTokensRangeParams) (*protocol.SemanticTokens, error) {
	return nil, nil
}

func (s *Server) SignatureHelp(ctx context.Context, params *protocol.SignatureHelpParams) (*protocol.SignatureHelp, error) {
	return nil, nil
}

func (s *Server) TypeDefinition(ctx context.Context, params *protocol.TypeDefinitionParams) (any, error) {
	return nil, nil
}

func (s *Server) WillSave(ctx context.Context, params *protocol.WillSaveTextDocumentParams) error {
	return nil
}

func (s *Server) WillSaveWaitUntil(ctx context.Context, params *protocol.WillSaveTextDocumentParams) ([]protocol.TextEdit, error) {
	return nil, nil
}

func (s *Server) Subtypes(ctx context.Context, params *protocol.TypeHierarchySubtypesParams) ([]protocol.TypeHierarchyItem, error) {
	return nil, nil
}

func (s *Server) Supertypes(ctx context.Context, params *protocol.TypeHierarchySupertypesParams) ([]protocol.TypeHierarchyItem, error) {
	return nil, nil
}

func (s *Server) WorkDoneProgressCancel(ctx context.Context, params *protocol.WorkDoneProgressCancelParams) error {
	return nil
}

func (s *Server) WorkspaceDiagnostic(ctx context.Context, params *protocol.WorkspaceDiagnosticParams) (*protocol.WorkspaceDiagnosticReport, error) {
	return nil, nil
}

func (s *Server) DidChangeConfiguration(ctx context.Context, params *protocol.DidChangeConfigurationParams) error {
	return nil
}

func (s *Server) DidChangeWatchedFiles(ctx context.Context, params *protocol.DidChangeWatchedFilesParams) error {
	return nil
}

func (s *Server) DidChangeWorkspaceFolders(ctx context.Context, params *protocol.DidChangeWorkspaceFoldersParams) error {
	return nil
}

func (s *Server) DidCreateFiles(ctx context.Context, params *protocol.CreateFilesParams) error {
	return nil
}

func (s *Server) DidDeleteFiles(ctx context.Context, params *protocol.DeleteFilesParams) error {
	return nil
}

func (s *Server) DidRenameFiles(ctx context.Context, params *protocol.RenameFilesParams) error {
	return nil
}

func (s *Server) ExecuteCommand(ctx context.Context, params *protocol.ExecuteCommandParams) (*protocol.LSPAny, error) {
	return nil, nil
}

func (s *Server) Symbols(ctx context.Context, params *protocol.WorkspaceSymbolParams) (any, error) {
	return nil, nil
}

func (s *Server) WillCreateFiles(ctx context.Context, params *protocol.CreateFilesParams) (*protocol.WorkspaceEdit, error) {
	return nil, nil
}

func (s *Server) WillDeleteFiles(ctx context.Context, params *protocol.DeleteFilesParams) (*protocol.WorkspaceEdit, error) {
	return nil, nil
}

func (s *Server) WillRenameFiles(ctx context.Context, params *protocol.RenameFilesParams) (*protocol.WorkspaceEdit, error) {
	return nil, nil
}

func (s *Server) WorkspaceSymbolResolve(ctx context.Context, params *protocol.WorkspaceSymbol) (*protocol.WorkspaceSymbol, error) {
	return nil, nil
}

func (s *Server) Request(ctx context.Context, method string, params any) (any, error) {
	return nil, nil
}

func RunServer(c *cache.Cache) error {
	server := NewServer(c)
	handler := protocol.ServerHandler(server, nil)

	stream := jsonrpc2.NewStream(&stdioReadWriteCloser{
		reader: os.Stdin,
		writer: os.Stdout,
	})
	conn := jsonrpc2.NewConn(stream)

	server.client = protocol.ClientDispatcher(conn, nil)

	conn.Go(context.Background(), handler)
	<-conn.Done()

	return conn.Err()
}
