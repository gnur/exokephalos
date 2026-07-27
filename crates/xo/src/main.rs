mod app;

use std::io::{self, Write as _, stdout};
use std::path::PathBuf;

use anyhow::{Context, Result};
use app::{App, Mode, PairingStep, external_edit_with, render, required_frontmatter};
use base64::Engine as _;
use clap::{Parser, Subcommand};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use time::OffsetDateTime;
use xo::config::{CliOverrides, XoConfig, config_path, home_dir};
use xo::session::WorkspaceSession;
use xo_core::domain::Frontmatter;
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
    let behavior = session.behavior().await?;
    let snapshot = session.snapshot().await?;
    let mut app = App::new(behavior, snapshot.notes.clone());
    app.workspace_id = session.workspace_id();
    hydrate(&mut app, &session, snapshot).await?;
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app, &mut session).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
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
        let key = match event::read()? {
            Event::Key(key) => key,
            Event::Paste(value) => {
                if let Some(pairing) = &mut app.pairing
                    && pairing.step == PairingStep::ServerOutput
                {
                    pairing.server_output.push_str(&value);
                    pairing.error.clear();
                }
                continue;
            }
            _ => continue,
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.mode {
            Mode::Search => match key.code {
                KeyCode::Esc | KeyCode::Enter => app.mode = Mode::Normal,
                KeyCode::Backspace => {
                    app.search.pop();
                    app.selected = 0;
                }
                KeyCode::Char(value) => {
                    app.search.push(value);
                    app.selected = 0;
                }
                _ => {}
            },
            Mode::CreateTitle => match key.code {
                KeyCode::Esc => {
                    app.create_title.clear();
                    app.mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    app.create_title.pop();
                }
                KeyCode::Enter => {
                    let title = app.create_title.trim().to_owned();
                    if title.is_empty() {
                        app.message = "a title is required".into();
                    } else {
                        app.create_title.clear();
                        app.mode = Mode::Normal;
                        suspend_tui(terminal)?;
                        let create_result = create_note(app, session, &title).await;
                        resume_tui(terminal)?;
                        create_result?;
                    }
                }
                KeyCode::Char(value) => app.create_title.push(value),
                _ => {}
            },
            Mode::Goto => match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => {
                    if app.choose_goto() {
                        app.mode = Mode::Normal;
                    }
                }
                KeyCode::Down => {
                    let last = app.goto_choices().len().saturating_sub(1);
                    app.goto_index = (app.goto_index + 1).min(last);
                }
                KeyCode::Up => {
                    app.goto_index = app.goto_index.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    app.goto_input.pop();
                    app.goto_index = 0;
                }
                KeyCode::Char(value) => {
                    app.goto_input.extend(value.to_lowercase());
                    app.goto_index = 0;
                    if app.goto_is_unambiguous() && app.choose_goto() {
                        app.mode = Mode::Normal;
                    }
                }
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
            Mode::Pairing => {
                let step = app.pairing.as_ref().map(|pairing| pairing.step);
                match (step, key.code) {
                    (_, KeyCode::Esc) => {
                        if step == Some(PairingStep::Connected) {
                            app.message = "sync server connected".into();
                        }
                        app.cancel_server_pairing();
                    }
                    (Some(PairingStep::StateDirectory), KeyCode::Backspace) => {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.state_dir.pop();
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::StateDirectory), KeyCode::Char('u'))
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.state_dir.clear();
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::StateDirectory), KeyCode::Char(value))
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.state_dir.push(value);
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::StateDirectory), KeyCode::Enter) => {
                        let state_dir = app
                            .pairing
                            .as_ref()
                            .map(|pairing| pairing.state_dir.trim())
                            .unwrap_or_default();
                        if state_dir.is_empty() {
                            if let Some(pairing) = &mut app.pairing {
                                pairing.error = "server state directory is required".into();
                            }
                        } else {
                            match session.writable_invitation().await {
                                Ok(invitation) => app.set_pairing_invitation(invitation),
                                Err(error) => {
                                    if let Some(pairing) = &mut app.pairing {
                                        pairing.error = error.to_string();
                                    }
                                }
                            }
                        }
                    }
                    (
                        Some(PairingStep::ServerCommand | PairingStep::ServerOutput),
                        KeyCode::F(2),
                    ) => {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.reveal_ticket = !pairing.reveal_ticket;
                        }
                    }
                    (Some(PairingStep::ServerCommand), KeyCode::Char('c')) => {
                        if let Some(command) = app.pairing_command() {
                            match copy_to_clipboard(terminal, &command) {
                                Ok(()) => app.message = "pairing commands copied".into(),
                                Err(error) => {
                                    if let Some(pairing) = &mut app.pairing {
                                        pairing.error = format!("could not copy commands: {error}");
                                    }
                                }
                            }
                        }
                    }
                    (Some(PairingStep::ServerCommand), KeyCode::Enter) => {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.step = PairingStep::ServerOutput;
                            pairing.server_output.clear();
                            pairing.reveal_ticket = false;
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::ServerOutput), KeyCode::Backspace) => {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.server_output.pop();
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::ServerOutput), KeyCode::Char(value))
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        if let Some(pairing) = &mut app.pairing {
                            pairing.server_output.push(value);
                            pairing.error.clear();
                        }
                    }
                    (Some(PairingStep::ServerOutput), KeyCode::Enter) => {
                        let Some(ticket) = app.pairing_ticket() else {
                            if let Some(pairing) = &mut app.pairing {
                                pairing.error = "paste the ticket= line printed by xo-admin".into();
                            }
                            continue;
                        };
                        match session.connect_peer(&ticket).await {
                            Ok(()) => {
                                if let Some(pairing) = &mut app.pairing {
                                    pairing.step = PairingStep::Connected;
                                    pairing.server_output.clear();
                                    pairing.invitation = None;
                                    pairing.error.clear();
                                }
                                app.sync = Some(session.sync_state.status()?);
                                app.message = "sync server connected".into();
                            }
                            Err(error) => {
                                if let Some(pairing) = &mut app.pairing {
                                    pairing.error = error.to_string();
                                }
                            }
                        }
                    }
                    (Some(PairingStep::Connected), KeyCode::Enter) => {
                        app.cancel_server_pairing();
                    }
                    _ => {}
                }
            }
            _ => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Tab => app.next_pane(),
                KeyCode::BackTab => app.previous_pane(),
                KeyCode::Down | KeyCode::Char('j') => match app.pane {
                    app::Pane::Tags => app.select_next_tag(),
                    _ => app.select_next(),
                },
                KeyCode::Up | KeyCode::Char('k') => match app.pane {
                    app::Pane::Tags => app.select_previous_tag(),
                    _ => app.select_previous(),
                },
                KeyCode::Char(' ') | KeyCode::Enter if app.pane == app::Pane::Tags => {
                    app.toggle_highlighted_tag();
                }
                KeyCode::Char('/') => {
                    app.mode = Mode::Search;
                }
                KeyCode::Char('g') => {
                    app.goto_input.clear();
                    app.goto_index = 0;
                    app.mode = Mode::Goto;
                }
                KeyCode::Char('a') => {
                    app.action_query.clear();
                    app.mode = Mode::ActionPicker;
                }
                KeyCode::Char('s') => app.toggle_sort(),
                KeyCode::Char('T') => app.toggle_tags_visible(),
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
                KeyCode::Char('J') => {
                    app.start_server_pairing();
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
                KeyCode::Char('c') => {
                    app.create_title.clear();
                    app.mode = Mode::CreateTitle;
                }
                KeyCode::Char('e') | KeyCode::Enter => {
                    suspend_tui(terminal)?;
                    let edit_result = edit_note(app, session).await;
                    resume_tui(terminal)?;
                    edit_result?;
                }
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
                KeyCode::Char('p') => {
                    suspend_tui(terminal)?;
                    let unlock_result = unlock(app);
                    resume_tui(terminal)?;
                    unlock_result?;
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

async fn create_note(app: &mut App, session: &mut WorkspaceSession, title: &str) -> Result<()> {
    let instant = OffsetDateTime::now_utc();
    let mut note = new_note_draft(instant, title)?;
    let initial = xo_core::markdown::render(&note.frontmatter, &note.body)?;
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let bytes = external_edit_with(&editor, &[], initial.as_bytes())?;
    let parsed = xo_core::markdown::parse(&String::from_utf8(bytes)?)?;
    let created = match note.frontmatter.get("created") {
        Some(xo_core::domain::FrontmatterValue::String(value)) => value.clone(),
        _ => unreachable!("new note drafts always have a creation timestamp"),
    };
    note.frontmatter = required_frontmatter(
        parsed.frontmatter.unwrap_or_default(),
        note.id.as_str(),
        &created,
    );
    note.body = parsed.body;
    app.search.clear();
    app.selected_tags.clear();
    app.set_view("all");
    app.add_note(note.clone());
    session.save(&note).await?;
    app.message = format!("created {}", note.id);
    Ok(())
}

fn new_note_draft(instant: OffsetDateTime, title: &str) -> Result<Note> {
    use time::format_description::well_known::Rfc3339;

    let created = instant.format(&Rfc3339)?;
    let id = xo_core::id::generate(instant);
    let mut frontmatter = Frontmatter::new();
    frontmatter.insert(
        "title".into(),
        xo_core::domain::FrontmatterValue::String(title.into()),
    );
    Ok(Note {
        id: NoteId::new(id.clone()),
        frontmatter: required_frontmatter(frontmatter, &id, &created),
        body: String::new(),
        path: format!("notes/{id}.md"),
    })
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
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

fn suspend_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableBracketedPaste
    )?;
    enable_raw_mode()?;
    terminal.clear()?;
    Ok(())
}

fn copy_to_clipboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    value: &str,
) -> Result<()> {
    let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(value));
    write!(
        terminal.backend_mut(),
        "\u{1b}]52;c;{}\u{7}",
        encoded.as_str()
    )?;
    terminal.backend_mut().flush()?;
    Ok(())
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

    #[test]
    fn new_note_draft_uses_the_prompted_title_and_required_frontmatter() {
        let instant = OffsetDateTime::from_unix_timestamp(1_750_000_000).unwrap();
        let note = new_note_draft(instant, "My title").unwrap();
        assert_eq!(
            note.frontmatter.get("title"),
            Some(&xo_core::domain::FrontmatterValue::String(
                "My title".into()
            ))
        );
        for field in ["id", "created", "tags", "title", "type"] {
            assert!(note.frontmatter.contains_key(field));
        }
        assert!(note.body.is_empty());
    }
}
