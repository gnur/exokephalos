package lsp

import (
	"context"
	"io"
	"os"
	"sync"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/config"
	"github.com/gnur/exokephalos/internal/repo"
	"go.lsp.dev/jsonrpc2"
	"go.lsp.dev/protocol"
	"go.lsp.dev/uri"
)

type stdioReadWriteCloser struct {
	reader io.Reader
	writer io.Writer
}

func (s *stdioReadWriteCloser) Read(p []byte) (n int, err error)  { return s.reader.Read(p) }
func (s *stdioReadWriteCloser) Write(p []byte) (n int, err error) { return s.writer.Write(p) }
func (s *stdioReadWriteCloser) Close() error                      { return nil }

type Server struct {
	protocol.UnimplementedServer
	client    protocol.Client
	cache     *cache.Cache
	config    *config.Config
	repo      *repo.Repo
	baseDir   string
	documents map[uri.URI]string
	mu        sync.RWMutex
}

func NewServer(c *cache.Cache, cfg *config.Config, baseDir string) *Server {
	return &Server{cache: c, config: cfg, repo: repo.New(baseDir, c), baseDir: baseDir, documents: make(map[uri.URI]string)}
}

func (s *Server) Initialize(ctx context.Context, params *protocol.InitializeParams) (*protocol.InitializeResult, error) {
	openClose, includeText, resolveSupport, prepareProvider := true, true, true, true
	changeKind := protocol.TextDocumentSyncKindFull
	version := "1.0.0"
	return &protocol.InitializeResult{
		Capabilities: protocol.ServerCapabilities{
			TextDocumentSync: &protocol.TextDocumentSyncOptions{
				OpenClose: &openClose,
				Change:    &changeKind,
				Save:      &protocol.SaveOptions{IncludeText: &includeText},
			},
			CompletionProvider: &protocol.CompletionOptions{TriggerCharacters: []string{"[", ":", "-", "#"}},
			HoverProvider:      protocol.Boolean(true),
			DefinitionProvider: protocol.Boolean(true),
			CodeActionProvider: &protocol.CodeActionOptions{ResolveProvider: &resolveSupport},
			InlayHintProvider:  protocol.Boolean(true),
			SemanticTokensProvider: &protocol.SemanticTokensOptions{
				Legend: SemanticTokenLegend,
				Full:   protocol.Boolean(true),
			},
			ReferencesProvider:      protocol.Boolean(true),
			RenameProvider:          &protocol.RenameOptions{PrepareProvider: &prepareProvider},
			DocumentSymbolProvider:  protocol.Boolean(true),
			DocumentLinkProvider:    &protocol.DocumentLinkOptions{},
			WorkspaceSymbolProvider: protocol.Boolean(true),
		},
		ServerInfo: protocol.ServerInfo{Name: "exokephalos-lsp", Version: protocol.NewOptional(version)},
	}, nil
}

func (s *Server) Shutdown(context.Context) error { return nil }

func (s *Server) DidOpen(ctx context.Context, params *protocol.DidOpenTextDocumentParams) error {
	s.mu.Lock()
	s.documents[params.TextDocument.URI] = params.TextDocument.Text
	s.mu.Unlock()
	s.publishDiagnostics(params.TextDocument.URI, params.TextDocument.Text)
	return nil
}

func (s *Server) DidChange(ctx context.Context, params *protocol.DidChangeTextDocumentParams) error {
	if len(params.ContentChanges) == 0 {
		return nil
	}
	change := params.ContentChanges[len(params.ContentChanges)-1]
	var text string
	switch change := change.(type) {
	case *protocol.TextDocumentContentChangeWholeDocument:
		text = change.Text
	case *protocol.TextDocumentContentChangePartial:
		// This server advertises full document synchronization, so clients must
		// not send incremental changes.
		return nil
	default:
		return nil
	}
	s.mu.Lock()
	s.documents[params.TextDocument.URI] = text
	s.mu.Unlock()
	s.publishDiagnostics(params.TextDocument.URI, text)
	return nil
}

func (s *Server) DidClose(ctx context.Context, params *protocol.DidCloseTextDocumentParams) error {
	s.mu.Lock()
	delete(s.documents, params.TextDocument.URI)
	s.mu.Unlock()
	s.publishDiagnostics(params.TextDocument.URI, "")
	return nil
}

func (s *Server) DidSave(ctx context.Context, params *protocol.DidSaveTextDocumentParams) error {
	if params.Text == nil {
		return nil
	}
	s.mu.Lock()
	s.documents[params.TextDocument.URI] = *params.Text
	s.mu.Unlock()
	s.publishDiagnostics(params.TextDocument.URI, *params.Text)
	return nil
}

func (s *Server) Completion(ctx context.Context, params *protocol.CompletionParams) (protocol.CompletionResult, error) {
	text, ok := s.document(params.TextDocument.URI)
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
	return &protocol.CompletionList{IsIncomplete: false, Items: items}, nil
}

func (s *Server) Hover(ctx context.Context, params *protocol.HoverParams) (*protocol.Hover, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return GetHover(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) Definition(ctx context.Context, params *protocol.DefinitionParams) (protocol.DefinitionResult, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return GetDefinition(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) CodeAction(ctx context.Context, params *protocol.CodeActionParams) ([]protocol.CommandOrCodeAction, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	actions, err := GetCodeActions(ctx, text, int(params.Range.Start.Line), params.TextDocument.URI)
	if err != nil {
		return nil, err
	}
	if action := s.missingNoteAction(text, int(params.Range.Start.Line), int(params.Range.Start.Character)); action != nil {
		actions = append(actions, *action)
	}
	if action := s.frontmatterAction(text, params.TextDocument.URI); action != nil {
		actions = append(actions, *action)
	}
	result := make([]protocol.CommandOrCodeAction, len(actions))
	for i := range actions {
		result[i] = &actions[i]
	}
	return result, nil
}

func (s *Server) CodeActionResolve(ctx context.Context, params *protocol.CodeAction) (*protocol.CodeAction, error) {
	return s.resolveCodeAction(ctx, params)
}

func (s *Server) InlayHint(ctx context.Context, params *protocol.InlayHintParams) ([]protocol.InlayHint, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return GetInlayHints(ctx, s.cache, text, int(params.Range.Start.Line), int(params.Range.End.Line))
}

func (s *Server) PrepareRename(ctx context.Context, params *protocol.PrepareRenameParams) (protocol.PrepareRenameResult, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return PrepareRename(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) References(ctx context.Context, params *protocol.ReferenceParams) ([]protocol.Location, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return GetReferences(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character))
}

func (s *Server) Rename(ctx context.Context, params *protocol.RenameParams) (*protocol.WorkspaceEdit, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return ReworkEdits(ctx, s.cache, text, int(params.Position.Line), int(params.Position.Character), params.NewName)
}

func (s *Server) SemanticTokensFull(ctx context.Context, params *protocol.SemanticTokensParams) (*protocol.SemanticTokens, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return &protocol.SemanticTokens{Data: GetSemanticTokens(text)}, nil
}

func (s *Server) DocumentSymbol(ctx context.Context, params *protocol.DocumentSymbolParams) (protocol.DocumentSymbolResult, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return protocol.DocumentSymbolSlice(documentSymbols(text)), nil
}

func (s *Server) DocumentLink(ctx context.Context, params *protocol.DocumentLinkParams) ([]protocol.DocumentLink, error) {
	text, ok := s.document(params.TextDocument.URI)
	if !ok {
		return nil, nil
	}
	return s.documentLinks(text, params.TextDocument.URI), nil
}

func (s *Server) Symbols(ctx context.Context, params *protocol.WorkspaceSymbolParams) (protocol.WorkspaceSymbolResult, error) {
	return s.workspaceSymbols(params.Query)
}

func (s *Server) document(documentURI uri.URI) (string, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	text, ok := s.documents[documentURI]
	return text, ok
}

func RunServer(c *cache.Cache, cfg *config.Config, baseDir string) error {
	server := NewServer(c, cfg, baseDir)
	stream := jsonrpc2.NewStream(&stdioReadWriteCloser{reader: os.Stdin, writer: os.Stdout})
	_, conn, client := protocol.NewServer(context.Background(), server, stream)
	server.client = client
	<-conn.Done()
	return conn.Err()
}
