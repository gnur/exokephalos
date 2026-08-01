use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams, InitializeResult,
    InitializedParams, MessageType, OneOf, Position, Range, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};
use xo_core::domain::FrontmatterValue;

#[derive(Debug, Parser)]
#[command(
    name = "xo-lsp",
    version = xo_core::version::VERSION,
    about = "Editor integration for exokephalos"
)]
struct Cli {
    /// Markdown projection to load when the editor does not provide a workspace root.
    #[arg(long)]
    workspace: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexedNote {
    uri: Url,
    id: String,
    title: String,
    tags: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct WorkspaceIndex {
    notes: Vec<IndexedNote>,
    diagnostics: HashMap<Url, Vec<Diagnostic>>,
}

impl WorkspaceIndex {
    fn load(root: &Path) -> std::io::Result<Self> {
        let mut markdown = Vec::new();
        collect_markdown(root, root, &mut markdown)?;
        markdown.sort();
        let mut index = Self::default();
        for path in markdown {
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            match std::fs::read_to_string(&path) {
                Ok(content) => index.replace_document(uri, &content),
                Err(error) => index
                    .diagnostics
                    .entry(uri)
                    .or_default()
                    .push(diagnostic(format!(
                        "could not read Markdown document: {error}"
                    ))),
            }
        }
        index.add_duplicate_id_diagnostics();
        Ok(index)
    }

    fn replace_document(&mut self, uri: Url, content: &str) {
        self.notes.retain(|note| note.uri != uri);
        self.diagnostics.remove(&uri);
        match parse_note(uri.clone(), content) {
            Ok(note) => self.notes.push(note),
            Err(message) => {
                self.diagnostics
                    .entry(uri)
                    .or_default()
                    .push(diagnostic(message));
            }
        }
    }

    fn add_duplicate_id_diagnostics(&mut self) {
        self.diagnostics.retain(|_, diagnostics| {
            diagnostics.retain(|item| !item.message.starts_with("duplicate note ID:"));
            !diagnostics.is_empty()
        });
        let mut by_id = HashMap::<String, Vec<Url>>::new();
        for note in &self.notes {
            by_id
                .entry(note.id.clone())
                .or_default()
                .push(note.uri.clone());
        }
        for (id, uris) in by_id {
            if uris.len() > 1 {
                for uri in uris {
                    self.diagnostics
                        .entry(uri)
                        .or_default()
                        .push(diagnostic(format!("duplicate note ID: {id}")));
                }
            }
        }
    }

    fn completions(&self, content: &str, position: Position) -> Vec<CompletionItem> {
        let offset = position_to_offset(content, position);
        let prefix = &content[..offset];
        if prefix
            .rsplit_once("[[")
            .is_some_and(|(_, value)| !value.contains("]]"))
        {
            let mut notes = self.notes.clone();
            notes.sort_by(|left, right| {
                left.title
                    .to_lowercase()
                    .cmp(&right.title.to_lowercase())
                    .then_with(|| left.id.cmp(&right.id))
            });
            return notes
                .into_iter()
                .map(|note| CompletionItem {
                    label: note.title,
                    detail: Some(note.id.clone()),
                    insert_text: Some(note.id),
                    kind: Some(CompletionItemKind::REFERENCE),
                    ..CompletionItem::default()
                })
                .collect();
        }
        self.notes
            .iter()
            .flat_map(|note| note.tags.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|tag| CompletionItem {
                label: tag.clone(),
                insert_text: Some(tag),
                kind: Some(CompletionItemKind::VALUE),
                ..CompletionItem::default()
            })
            .collect()
    }
}

fn parse_note(uri: Url, content: &str) -> Result<IndexedNote, String> {
    let document = xo_core::markdown::parse(content).map_err(|error| error.to_string())?;
    let frontmatter = document
        .frontmatter
        .ok_or_else(|| "Markdown document has no YAML frontmatter".to_owned())?;
    let id = match frontmatter.get("id") {
        Some(FrontmatterValue::String(id)) if xo_core::id::is_valid(id) => id.clone(),
        Some(FrontmatterValue::String(id)) => return Err(format!("invalid note ID: {id}")),
        _ => return Err("frontmatter has no string id".to_owned()),
    };
    let title = match frontmatter.get("title") {
        Some(FrontmatterValue::String(title)) => title.clone(),
        _ => "Untitled".to_owned(),
    };
    let tags = xo_core::markdown::tags(&frontmatter)
        .into_iter()
        .map(str::to_owned)
        .collect();
    Ok(IndexedNote {
        uri,
        id,
        title,
        tags,
    })
}

fn collect_markdown(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if path != root
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_markdown(root, &path, paths)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "md")
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 1)),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("xo".to_owned()),
        message,
        ..Diagnostic::default()
    }
}

fn position_to_offset(content: &str, position: Position) -> usize {
    let line = content
        .split_inclusive('\n')
        .nth(position.line as usize)
        .unwrap_or_default();
    let line_start = content.len().saturating_sub(
        content
            .split_inclusive('\n')
            .skip(position.line as usize)
            .map(str::len)
            .sum::<usize>(),
    );
    let mut utf16 = 0_u32;
    let mut bytes = 0_usize;
    for character in line.chars() {
        if character == '\n' || utf16 >= position.character {
            break;
        }
        let units = u32::try_from(character.len_utf16()).unwrap_or(2);
        if utf16 + units > position.character {
            break;
        }
        utf16 += units;
        bytes += character.len_utf8();
    }
    (line_start + bytes).min(content.len())
}

#[derive(Debug, Default)]
struct ServerState {
    root: Option<PathBuf>,
    index: WorkspaceIndex,
    open_documents: HashMap<Url, String>,
    published_diagnostic_uris: HashSet<Url>,
}

struct Backend {
    client: Client,
    fallback_root: Option<PathBuf>,
    state: Mutex<ServerState>,
}

impl Backend {
    fn new(client: Client, fallback_root: Option<PathBuf>) -> Self {
        Self {
            client,
            fallback_root,
            state: Mutex::new(ServerState::default()),
        }
    }

    async fn rebuild(&self) {
        let mut state = self.state.lock().await;
        let Some(root) = state.root.clone() else {
            return;
        };
        let mut index = WorkspaceIndex::load(&root).unwrap_or_default();
        for (uri, content) in &state.open_documents {
            index.replace_document(uri.clone(), content);
        }
        index.add_duplicate_id_diagnostics();
        state.index = index;
    }

    async fn publish_diagnostics(&self) {
        let diagnostics = {
            let mut state = self.state.lock().await;
            let current_uris = state
                .index
                .diagnostics
                .keys()
                .cloned()
                .collect::<HashSet<_>>();
            let uris = state
                .published_diagnostic_uris
                .union(&current_uris)
                .cloned()
                .collect::<Vec<_>>();
            state.published_diagnostic_uris = current_uris;
            uris.into_iter()
                .map(|uri| {
                    let diagnostics = state
                        .index
                        .diagnostics
                        .get(&uri)
                        .cloned()
                        .unwrap_or_default();
                    (uri, diagnostics)
                })
                .collect::<Vec<_>>()
        };
        for (uri, diagnostics) in diagnostics {
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        let root = params
            .workspace_folders
            .and_then(|folders| folders.first().map(|folder| folder.uri.clone()))
            .or(params.root_uri)
            .and_then(|uri| uri.to_file_path().ok())
            .or_else(|| self.fallback_root.clone());
        self.state.lock().await.root = root;
        self.rebuild().await;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["[".to_owned(), "#".to_owned()]),
                    ..CompletionOptions::default()
                }),
                workspace_symbol_provider: Some(OneOf::Left(false)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "xo-lsp".to_owned(),
                version: Some(xo_core::version::VERSION.to_owned()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.publish_diagnostics().await;
        self.client
            .log_message(MessageType::INFO, "xo workspace loaded")
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        self.state
            .lock()
            .await
            .open_documents
            .insert(document.uri.clone(), document.text);
        self.rebuild().await;
        self.publish_diagnostics().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.state
                .lock()
                .await
                .open_documents
                .insert(params.text_document.uri, change.text);
            self.rebuild().await;
            self.publish_diagnostics().await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(text) = params.text {
            self.state.lock().await.open_documents.insert(uri, text);
        } else {
            self.state.lock().await.open_documents.remove(&uri);
        }
        self.rebuild().await;
        self.publish_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.state
            .lock()
            .await
            .open_documents
            .remove(&params.text_document.uri);
        self.rebuild().await;
        self.publish_diagnostics().await;
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let state = self.state.lock().await;
        let uri = &params.text_document_position.text_document.uri;
        let content = state.open_documents.get(uri).map_or("", String::as_str);
        let items = state
            .index
            .completions(content, params.text_document_position.position);
        Ok(Some(CompletionResponse::Array(items)))
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend::new(client, cli.workspace));
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, tags: &str) -> String {
        format!("---\nid: {id}\ntitle: {title}\ntype: note\ntags: [{tags}]\n---\nbody")
    }

    #[test]
    fn workspace_loading_indexes_notes_tags_and_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("valid.md"),
            note("abcdefg", "Alpha", "work, shared"),
        )
        .unwrap();
        std::fs::write(directory.path().join("invalid.md"), "---\nid: bad\n---\n").unwrap();
        std::fs::write(directory.path().join("malformed.md"), "---\ntags: [\n---\n").unwrap();
        std::fs::create_dir(directory.path().join(".hidden")).unwrap();
        std::fs::write(
            directory.path().join(".hidden/ignored.md"),
            note("bcdefgh", "Ignored", "private"),
        )
        .unwrap();

        let index = WorkspaceIndex::load(directory.path()).unwrap();

        assert_eq!(index.notes.len(), 1);
        assert_eq!(index.notes[0].title, "Alpha");
        assert_eq!(
            index.notes[0].tags,
            BTreeSet::from(["shared".into(), "work".into()])
        );
        assert!(
            index
                .diagnostics
                .values()
                .flatten()
                .any(|item| item.message.contains("invalid note ID"))
        );
        assert!(
            index
                .diagnostics
                .values()
                .flatten()
                .any(|item| item.message.contains("invalid YAML frontmatter"))
        );
    }

    #[test]
    fn completion_distinguishes_wikilinks_from_tags_and_uses_utf16_positions() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("valid.md"),
            note("abcdefg", "Alpha", "work"),
        )
        .unwrap();
        let index = WorkspaceIndex::load(directory.path()).unwrap();

        let links = index.completions("😀 [[", Position::new(0, 5));
        assert_eq!(links[0].label, "Alpha");
        assert_eq!(links[0].insert_text.as_deref(), Some("abcdefg"));
        let tags = index.completions("tags: [", Position::new(0, 7));
        assert_eq!(tags[0].label, "work");
    }

    #[test]
    fn duplicate_ids_are_diagnosed_for_every_document() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("one.md"), note("abcdefg", "One", "")).unwrap();
        std::fs::write(directory.path().join("two.md"), note("abcdefg", "Two", "")).unwrap();

        let index = WorkspaceIndex::load(directory.path()).unwrap();

        assert_eq!(
            index
                .diagnostics
                .values()
                .flatten()
                .filter(|item| item.message.contains("duplicate note ID"))
                .count(),
            2
        );
    }
}
