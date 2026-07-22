mod app;
mod config;
mod session;

use std::io::{self, stdout};
use std::path::PathBuf;

use anyhow::{Context, Result};
use app::{App, Mode, external_edit_with, render};
use clap::{Parser, Subcommand};
use config::{CliOverrides, XoConfig, config_path, home_dir};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use session::WorkspaceSession;
use time::OffsetDateTime;
use xo_core::behavior::TemplateInputs;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::{Note, NoteId};
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(
    name = "xo",
    version,
    about = "Offline-first personal knowledge workspace"
)]
struct Cli {
    /// Override the persistent Iroh state directory from config.scm.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Override the workspace ID from config.scm.
    #[arg(long, conflicts_with = "ticket")]
    workspace: Option<String>,
    /// Import/connect a ticket instead of opening the configured workspace.
    #[arg(long, conflicts_with = "workspace")]
    ticket: Option<String>,
    /// Override the local Markdown projection directory from config.scm.
    #[arg(long)]
    projection: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print a default ~/.config/xo/config.scm document to stdout.
    ConfigInit,
    /// Validate that a Markdown document can be read by the Rust core.
    Validate { path: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::ConfigInit) => print!("{}", XoConfig::default().document()?),
        Some(Command::Validate { path }) => {
            let content = std::fs::read_to_string(&path)?;
            let document = xo_core::markdown::parse(&content)?;
            println!(
                "{}: valid (frontmatter={}, body_bytes={})",
                path.display(),
                document.frontmatter.is_some(),
                document.body.len()
            );
        }
        None => {
            let home = home_dir()?;
            let ticket = cli.ticket;
            let config = XoConfig::load(&config_path(&home), &home)?.apply(
                CliOverrides {
                    state_dir: cli.state_dir,
                    workspace: cli.workspace,
                    projection: cli.projection,
                },
                &home,
            );
            run_tui(
                &config.state_dir,
                config.workspace.as_deref(),
                ticket.as_deref(),
                config.projection,
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_tui(
    state_dir: &std::path::Path,
    workspace: Option<&str>,
    ticket: Option<&str>,
    projection: PathBuf,
) -> Result<()> {
    let mut session = WorkspaceSession::open(state_dir, workspace, ticket, projection).await?;
    let snapshot = session.snapshot().await?;
    let behavior = session.behavior().await?;
    let mut app = App::new(behavior, snapshot.notes.clone());
    app.message = format!("workspace {}", session.workspace_id());
    hydrate(&mut app, &session, snapshot).await?;
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app, &mut session).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    session.shutdown().await?;
    result
}

async fn hydrate(
    app: &mut App,
    session: &WorkspaceSession,
    snapshot: xo_core::records::WorkspaceSnapshot,
) -> Result<()> {
    app.notes = snapshot.notes;
    app.conflicts = snapshot
        .resolved
        .iter()
        .filter_map(|value| value.conflict.clone())
        .collect();
    app.conflict_history.clear();
    for conflict in &app.conflicts {
        app.conflict_history.insert(
            conflict.note_id.clone(),
            session.history(&conflict.note_id).await?,
        );
    }
    app.deleted = session
        .deleted_notes()
        .await?
        .into_iter()
        .map(|note| (note.id.clone(), note))
        .collect();
    app.devices = snapshot.devices;
    app.diagnostics = snapshot.diagnostics;
    app.operations = session.sync_state.ready()?;
    app.sync = Some(session.sync_state.status()?);
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &mut WorkspaceSession,
) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.mode {
            Mode::Search => match key.code {
                KeyCode::Esc | KeyCode::Enter => app.mode = Mode::Normal,
                KeyCode::Backspace => {
                    app.search.pop();
                }
                KeyCode::Char(value) => app.search.push(value),
                _ => {}
            },
            Mode::ActionPicker => match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Backspace => {
                    app.action_query.pop();
                }
                KeyCode::Char(value) => app.action_query.push(value),
                KeyCode::Enter => {
                    let id = app.matching_actions().first().map(|value| value.id.clone());
                    if let Some(id) = id {
                        let note = app.run_action(&id)?;
                        session.save(&note).await?;
                        app.message = format!("applied {id}");
                    }
                    app.mode = Mode::Normal;
                }
                _ => {}
            },
            _ => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Tab => app.next_pane(),
                KeyCode::BackTab => app.previous_pane(),
                KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
                KeyCode::Char('/') => {
                    app.search.clear();
                    app.mode = Mode::Search;
                }
                KeyCode::Char('a') => {
                    app.action_query.clear();
                    app.mode = Mode::ActionPicker;
                }
                KeyCode::Char('s') => app.toggle_sort(),
                KeyCode::Char('t') => {
                    let tags = selected_tags(app);
                    for tag in tags {
                        app.toggle_tag(&tag);
                    }
                }
                KeyCode::Char(']') => cycle_subview(app),
                KeyCode::Char('x') => {
                    app.mode = Mode::Conflicts;
                    app.message = conflict_summary(app);
                }
                KeyCode::Char('v') => {
                    app.mode = Mode::Devices;
                    app.message = device_summary(app);
                }
                KeyCode::Char('y') => {
                    app.mode = Mode::Sync;
                    app.message = sync_summary(app);
                }
                KeyCode::Char('r') => {
                    session.refresh_sync()?;
                    let snapshot = session.snapshot().await?;
                    hydrate(app, session, snapshot).await?;
                    app.message = "refreshed and retried synchronization".into();
                }
                KeyCode::Char('R') => {
                    if let Some(operation) = app.operations.first() {
                        session.retry(operation.id)?;
                        app.message = format!("queued retry {}", operation.id);
                    }
                }
                KeyCode::Char('c') => create_note(app, session).await?,
                KeyCode::Char('e') => edit_note(app, session).await?,
                KeyCode::Char('d') => {
                    if let Some(note) = app.selected_note().cloned() {
                        session.delete(&note).await?;
                        app.delete_selected();
                        app.message = format!("deleted {} (u restores)", note.id);
                    }
                }
                KeyCode::Char('u') => {
                    if let Some(id) = app.deleted.keys().next_back().cloned()
                        && let Some(note) = app.restore(&id)
                    {
                        session.save(&note).await?;
                        app.message = format!("restored {id}");
                    }
                }
                KeyCode::Char('V') => {
                    if let Some(device) = app
                        .devices
                        .iter()
                        .find(|device| device.retired_at.is_none())
                        .cloned()
                    {
                        session.retire_device(device.clone()).await?;
                        app.message = format!("retired device {}", device.endpoint_id);
                    }
                }
                KeyCode::Char('p') => unlock(app)?,
                KeyCode::Char(value) if key.modifiers.is_empty() => {
                    if let Some(view) = app
                        .behavior
                        .views
                        .iter()
                        .find(|view| view.key.as_deref() == Some(&value.to_string()))
                    {
                        let id = view.id.clone();
                        app.set_view(&id);
                    } else if value == '0' {
                        app.set_view("all");
                    }
                }
                KeyCode::Esc => {
                    app.mode = Mode::Normal;
                    app.message.clear();
                }
                _ => {}
            },
        }
    }
    Ok(())
}

async fn create_note(app: &mut App, session: &mut WorkspaceSession) -> Result<()> {
    let instant = OffsetDateTime::now_utc();
    let id = xo_core::id::generate(instant);
    let path = format!("notes/{id}.md");
    let note = if let Some(template) = app.behavior.templates.first() {
        let inputs = TemplateInputs::deterministic(
            instant,
            id.clone(),
            id.clone(),
            std::collections::BTreeMap::default(),
        )?;
        app.create_from_template(&template.id.clone(), &inputs, path)?
    } else {
        let note = Note {
            id: NoteId::new(id.clone()),
            frontmatter: Frontmatter::from([
                ("id".into(), FrontmatterValue::String(id.clone())),
                ("title".into(), FrontmatterValue::String("Untitled".into())),
            ]),
            body: String::new(),
            path,
        };
        app.notes.push(note.clone());
        note
    };
    session.save(&note).await?;
    app.message = format!("created {}", note.id);
    Ok(())
}

async fn edit_note(app: &mut App, session: &mut WorkspaceSession) -> Result<()> {
    let note = app.selected_note().context("no selected note")?.clone();
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let edited = if xo_core::encryption::is_encrypted(&note.body) {
        let passphrase = password("Passphrase: ")?;
        app.edit_encrypted_with(&passphrase, &editor, &[])?
    } else {
        let document = xo_core::markdown::render(&note.frontmatter, &note.body)?;
        let bytes = external_edit_with(&editor, &[], document.as_bytes())?;
        let parsed = xo_core::markdown::parse(&String::from_utf8(bytes)?)?;
        app.replace_selected(parsed.frontmatter.unwrap_or_default(), parsed.body)
            .context("selected note disappeared")?
    };
    session.save(&edited).await?;
    app.message = format!("saved {}", edited.id);
    Ok(())
}

fn unlock(app: &mut App) -> Result<()> {
    if app
        .selected_note()
        .is_some_and(|note| xo_core::encryption::is_encrypted(&note.body))
    {
        let passphrase = password("Passphrase: ")?;
        app.unlock_preview(&passphrase)?;
    }
    Ok(())
}
fn password(prompt: &str) -> Result<Zeroizing<String>> {
    disable_raw_mode()?;
    let result = rpassword::prompt_password(prompt).map(Zeroizing::new);
    enable_raw_mode()?;
    Ok(result?)
}
fn selected_tags(app: &App) -> Vec<String> {
    app.selected_note()
        .and_then(|note| note.frontmatter.get("tags"))
        .map_or_else(Vec::new, |value| match value {
            FrontmatterValue::Sequence(values) => values
                .iter()
                .filter_map(|value| {
                    if let FrontmatterValue::String(value) = value {
                        Some(value.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            FrontmatterValue::String(value) => value
                .split(',')
                .map(|value| value.trim().to_owned())
                .collect(),
            _ => vec![],
        })
}
fn cycle_subview(app: &mut App) {
    let Some(view) = app
        .behavior
        .views
        .iter()
        .find(|view| view.id == app.active_view)
    else {
        return;
    };
    let next = match app
        .active_subview
        .as_ref()
        .and_then(|id| view.subviews.iter().position(|item| &item.id == id))
    {
        Some(index) if index + 1 < view.subviews.len() => Some(view.subviews[index + 1].id.clone()),
        None if !view.subviews.is_empty() => Some(view.subviews[0].id.clone()),
        _ => None,
    };
    app.set_subview(next);
}
fn conflict_summary(app: &App) -> String {
    if app.conflicts.is_empty() {
        "no conflicts".into()
    } else {
        app.conflicts
            .iter()
            .map(|value| {
                format!(
                    "{}: winner {}, alternatives {}, history {}",
                    value.note_id,
                    value.winning_revision,
                    value
                        .concurrent_revisions
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    app.conflict_history.get(&value.note_id).map_or(0, Vec::len)
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }
}
fn device_summary(app: &App) -> String {
    app.devices
        .iter()
        .map(|value| {
            format!(
                "{} {} retired={}",
                value.label,
                value.endpoint_id,
                value.retired_at.is_some()
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}
fn sync_summary(app: &App) -> String {
    format!(
        "operations: {}; missing blobs: {}",
        app.operations
            .iter()
            .map(|value| format!("{}:{:?}", value.id, value.status))
            .collect::<Vec<_>>()
            .join(", "),
        app.sync
            .as_ref()
            .map(|value| value.missing_blobs.join(", "))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn no_subcommand_selects_the_tui_mode() {
        let cli = Cli::try_parse_from(["xo"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn tui_flags_are_available_without_a_tui_subcommand() {
        let cli = Cli::try_parse_from([
            "xo",
            "--state-dir",
            "/state",
            "--workspace",
            "workspace-id",
            "--projection",
            "/notes",
        ])
        .unwrap();
        assert_eq!(
            cli.state_dir.as_deref(),
            Some(std::path::Path::new("/state"))
        );
        assert_eq!(cli.workspace.as_deref(), Some("workspace-id"));
        assert_eq!(
            cli.projection.as_deref(),
            Some(std::path::Path::new("/notes"))
        );
    }
}
