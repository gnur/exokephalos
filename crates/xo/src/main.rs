mod app;

use std::io::{self, Write as _, stdout};
use std::path::PathBuf;

use anyhow::{Context, Result};
use app::{App, Mode, PairingStep, external_edit_with, render, required_frontmatter};
use base64::Engine as _;
use clap::{Parser, Subcommand};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_lite::{Stream, StreamExt};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use time::OffsetDateTime;
use xo::config::{CliOverrides, XoConfig, config_path, home_dir};
use xo::session::WorkspaceSession;
use xo::steel_plugin::{PluginChoice, execute as execute_steel_plugin};
use xo::url_capture::{UrlCaptureService, captured_note};
use xo_core::behavior::ActionPlugin;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::iroh_node::WorkspaceEvent;
use xo_core::{Note, NoteId};
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(
    name = "xo",
    version = xo_core::version::VERSION,
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
    /// Recursively import Markdown into the configured active workspace.
    Import {
        source: PathBuf,
        #[arg(long = "type", default_value = "note")]
        item_type: String,
    },
    /// Export the configured active workspace as conventional Markdown.
    Export {
        destination: PathBuf,
        #[arg(long = "type")]
        item_type: Option<String>,
    },
    /// Install a bundled executable Steel plugin into the active workspace.
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    /// Install or update a bundled plugin.
    Install { name: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Some(Command::ConfigInit) => print!("{}", XoConfig::default().document()?),
        Some(Command::Validate { path }) => {
            let content = std::fs::read_to_string(path)?;
            let document = xo_core::markdown::parse(&content)?;
            println!(
                "{}: valid (frontmatter={}, body_bytes={})",
                path.display(),
                document.frontmatter.is_some(),
                document.body.len()
            );
        }
        Some(Command::Import { source, item_type }) => {
            let config = configured(&cli)?;
            let mut session = WorkspaceSession::open(
                &config.state_dir,
                config.workspace.as_deref(),
                cli.ticket.as_deref(),
                config.projection,
            )
            .await?;
            let result = xo::content_io::import_markdown(&mut session, source, item_type).await;
            let shutdown = session.shutdown().await;
            let imported = result?;
            shutdown?;
            println!("imported={imported}");
        }
        Some(Command::Export {
            destination,
            item_type,
        }) => {
            let config = configured(&cli)?;
            let session = WorkspaceSession::open(
                &config.state_dir,
                config.workspace.as_deref(),
                cli.ticket.as_deref(),
                config.projection,
            )
            .await?;
            let result =
                xo::content_io::export_markdown(&session, destination, item_type.as_deref()).await;
            let shutdown = session.shutdown().await;
            let exported = result?;
            shutdown?;
            println!("exported={}", exported.exported);
        }
        Some(Command::Plugin {
            command: PluginCommand::Install { name },
        }) => {
            let (path, source) = match name.as_str() {
                "hardcover" => (
                    "plugins/hardcover.scm",
                    include_bytes!("../../../plugins/hardcover.scm").as_slice(),
                ),
                _ => anyhow::bail!("unknown bundled plugin {name:?}"),
            };
            let config = configured(&cli)?;
            let mut session = WorkspaceSession::open(
                &config.state_dir,
                config.workspace.as_deref(),
                cli.ticket.as_deref(),
                config.projection,
            )
            .await?;
            // Establish the main workspace descriptor before adding a module,
            // so generated defaults never absorb and duplicate plugin actions.
            session.behavior().await?;
            session.install_config(path, source).await?;
            session.shutdown().await?;
            println!("installed {name} as {path}");
        }
        None => {
            let config = configured(&cli)?;
            run_tui(
                &config.state_dir,
                config.workspace.as_deref(),
                cli.ticket.as_deref(),
                config.projection,
                &config.pwa_url,
                &config.leader_key,
            )
            .await?;
        }
    }
    Ok(())
}

fn configured(cli: &Cli) -> Result<XoConfig> {
    let home = home_dir()?;
    Ok(XoConfig::load(&config_path(&home), &home)?.apply(
        CliOverrides {
            state_dir: cli.state_dir.clone(),
            workspace: cli.workspace.clone(),
            projection: cli.projection.clone(),
        },
        &home,
    ))
}

async fn run_tui(
    state_dir: &std::path::Path,
    workspace: Option<&str>,
    ticket: Option<&str>,
    projection: PathBuf,
    pwa_url: &str,
    leader_key: &str,
) -> Result<()> {
    let mut session = WorkspaceSession::open(state_dir, workspace, ticket, projection).await?;
    let workspace_events = session.subscribe().await?;
    let behavior = session.behavior().await?;
    let snapshot = session.snapshot().await?;
    let mut app = App::new(behavior, snapshot.notes.clone());
    app.workspace_id = session.workspace_id();
    pwa_url.clone_into(&mut app.pwa_url);
    app.leader_key = leader_key
        .chars()
        .next()
        .context("leader-key is unavailable")?;
    hydrate(&mut app, &session, snapshot).await?;
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app, &mut session, workspace_events).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    println!("Finalizing workspace state and shutting down synchronization...");
    io::stdout().flush()?;
    session.shutdown().await?;
    result
}

async fn hydrate(
    app: &mut App,
    session: &WorkspaceSession,
    snapshot: xo_core::records::WorkspaceSnapshot,
) -> Result<()> {
    let selected_note = app.selected_note().map(|note| note.id.clone());
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
    app.selected = selected_note
        .and_then(|id| app.visible_notes().iter().position(|note| note.id == id))
        .unwrap_or_else(|| app.selected_index().unwrap_or(0));
    Ok(())
}

async fn refresh_workspace(app: &mut App, session: &mut WorkspaceSession) -> Result<()> {
    let behavior = session.behavior().await?;
    let current_view_exists = behavior.views.iter().any(|view| view.id == app.active_view);
    app.behavior = behavior;
    if !current_view_exists {
        let default_view = app.behavior.default_view.clone();
        app.set_view(&default_view);
    } else if let Some(subview) = &app.active_subview
        && !app
            .behavior
            .views
            .iter()
            .find(|view| view.id == app.active_view)
            .is_some_and(|view| view.subviews.iter().any(|item| item.id == *subview))
    {
        app.set_subview(None);
    }
    let snapshot = session.snapshot().await?;
    hydrate(app, session, snapshot).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaderCommand {
    ToggleTags,
    ChooseView,
    Actions,
    Config,
    Mobile,
    Server,
    Sync,
    Conflicts,
    Devices,
    Refresh,
    ReverseSort,
    Unlock,
}

const fn leader_command(key: char) -> Option<LeaderCommand> {
    match key {
        't' => Some(LeaderCommand::ToggleTags),
        'v' => Some(LeaderCommand::ChooseView),
        'a' => Some(LeaderCommand::Actions),
        'c' => Some(LeaderCommand::Config),
        'm' => Some(LeaderCommand::Mobile),
        'j' => Some(LeaderCommand::Server),
        's' => Some(LeaderCommand::Sync),
        'x' => Some(LeaderCommand::Conflicts),
        'i' => Some(LeaderCommand::Devices),
        'r' => Some(LeaderCommand::Refresh),
        'o' => Some(LeaderCommand::ReverseSort),
        'p' => Some(LeaderCommand::Unlock),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &mut WorkspaceSession,
    mut workspace_events: impl Stream<Item = Result<WorkspaceEvent>> + Unpin,
) -> Result<()> {
    let mut terminal_events = EventStream::new();
    loop {
        terminal.draw(|frame| render(frame, app))?;
        let event = tokio::select! {
            terminal_event = terminal_events.next() => terminal_event
                .context("terminal event stream ended")?
                .context("read terminal event")?,
            workspace_event = workspace_events.next() => {
                match workspace_event {
                    Some(Ok(WorkspaceEvent::ContentChanged)) => {
                        match refresh_workspace(app, session).await {
                            Ok(()) if app.message.starts_with("automatic workspace refresh failed:") => {
                                app.message = "workspace updated".into();
                            }
                            Ok(()) => {}
                            Err(error) => {
                                app.message = format!("automatic workspace refresh failed: {error:#}");
                            }
                        }
                    }
                    Some(Ok(WorkspaceEvent::StatusChanged)) => {}
                    Some(Err(error)) => {
                        app.message = format!("workspace event stream failed: {error:#}");
                    }
                    None => anyhow::bail!("workspace event stream ended"),
                }
                continue;
            }
        };
        let key = match event {
            Event::Key(key) => key,
            Event::Paste(value) => {
                if app.mode == Mode::CaptureUrl {
                    app.capture_url.push_str(value.trim());
                } else if app.mode == Mode::PluginInput {
                    app.plugin_input.push_str(value.trim());
                } else if let Some(pairing) = &mut app.pairing
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
            Mode::Leader => match key.code {
                KeyCode::Char(value)
                    if leader_command(value) == Some(LeaderCommand::ToggleTags) =>
                {
                    app.toggle_tags_visible();
                    app.mode = Mode::Normal;
                }
                KeyCode::Char(value)
                    if leader_command(value) == Some(LeaderCommand::ChooseView) =>
                {
                    app.goto_input.clear();
                    app.goto_index = 0;
                    app.mode = Mode::Goto;
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Actions) => {
                    app.action_query.clear();
                    app.mode = Mode::ActionPicker;
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Config) => {
                    app.mode = Mode::Normal;
                    suspend_tui(terminal)?;
                    let config_result = edit_workspace_config(session).await;
                    resume_tui(terminal)?;
                    config_result?;
                    refresh_workspace(app, session).await?;
                    app.message = "workspace configuration updated".into();
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Mobile) => {
                    match session.writable_invitation().await {
                        Ok(ticket) => {
                            let ticket = Zeroizing::new(ticket);
                            if let Err(error) = app.start_mobile_pairing(&ticket) {
                                app.message = format!("could not create mobile setup: {error:#}");
                                app.mode = Mode::Normal;
                            }
                        }
                        Err(error) => {
                            app.message = format!("could not create invitation: {error:#}");
                            app.mode = Mode::Normal;
                        }
                    }
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Server) => {
                    app.start_server_pairing();
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Sync) => {
                    app.mode = Mode::Sync;
                    app.message = sync_summary(app);
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Conflicts) => {
                    app.mode = Mode::Conflicts;
                    app.message = conflict_summary(app);
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Devices) => {
                    app.mode = Mode::Devices;
                    app.message = device_summary(app);
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Refresh) => {
                    session.refresh_sync()?;
                    refresh_workspace(app, session).await?;
                    app.message = "refreshed and retried synchronization".into();
                    app.mode = Mode::Normal;
                }
                KeyCode::Char(value)
                    if leader_command(value) == Some(LeaderCommand::ReverseSort) =>
                {
                    app.toggle_sort();
                    app.mode = Mode::Normal;
                }
                KeyCode::Char(value) if leader_command(value) == Some(LeaderCommand::Unlock) => {
                    app.mode = Mode::Normal;
                    suspend_tui(terminal)?;
                    let unlock_result = unlock(app);
                    resume_tui(terminal)?;
                    unlock_result?;
                }
                KeyCode::Char(value) => {
                    app.message = format!("unknown leader command {value}");
                    app.mode = Mode::Normal;
                }
                _ => app.mode = Mode::Normal,
            },
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
            Mode::CreateTitle | Mode::CreateEncryptedTitle => match key.code {
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
                        let encrypted = app.mode == Mode::CreateEncryptedTitle;
                        app.create_title.clear();
                        app.mode = Mode::Normal;
                        suspend_tui(terminal)?;
                        let create_result = if encrypted {
                            create_encrypted_note(app, session, &title).await
                        } else {
                            create_note(app, session, &title).await
                        };
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
            Mode::ViewPicker => match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => {
                    if app.choose_main_view() {
                        app.mode = Mode::Normal;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = app.main_view_choices().len().saturating_sub(1);
                    app.goto_index = (app.goto_index + 1).min(last);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.goto_index = app.goto_index.saturating_sub(1);
                }
                KeyCode::Char(value) => {
                    if app.choose_main_view_key(value) {
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
                        match app
                            .behavior
                            .action(app.selected_note(), &id)
                            .map(|action| action.plugin.clone())
                        {
                            Ok(Some(ActionPlugin::CaptureUrl)) => {
                                app.capture_url.clear();
                                app.mode = Mode::CaptureUrl;
                            }
                            Ok(Some(ActionPlugin::Steel { prompt, .. })) => {
                                app.plugin_input.clear();
                                app.plugin_results.clear();
                                app.plugin_index = 0;
                                app.plugin_action = Some(id);
                                app.plugin_prompt = prompt;
                                app.mode = Mode::PluginInput;
                            }
                            Ok(None) => {
                                let note = app.run_action(&id)?;
                                session.save(&note).await?;
                                app.message = format!("applied {id}");
                                app.mode = Mode::Normal;
                            }
                            Err(error) => {
                                app.message = error.to_string();
                                app.mode = Mode::Normal;
                            }
                        }
                    } else {
                        app.mode = Mode::Normal;
                    }
                }
                _ => {}
            },
            Mode::CaptureUrl => match key.code {
                KeyCode::Esc => {
                    app.capture_url.clear();
                    app.mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    app.capture_url.pop();
                }
                KeyCode::Char(value)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    app.capture_url.push(value);
                }
                KeyCode::Enter => {
                    let raw_url = app.capture_url.trim().to_owned();
                    if raw_url.is_empty() {
                        app.message = "a URL is required".into();
                        continue;
                    }
                    suspend_tui(terminal)?;
                    let capture = capture_url(session, &raw_url).await;
                    resume_tui(terminal)?;
                    match capture {
                        Ok(note) => {
                            app.search.clear();
                            app.selected_tags.clear();
                            app.set_view("all");
                            app.add_note(note);
                            app.message = format!("captured {raw_url}");
                        }
                        Err(error) => app.message = format!("URL capture failed: {error:#}"),
                    }
                    app.capture_url.clear();
                    app.mode = Mode::Normal;
                }
                _ => {}
            },
            Mode::PluginInput => match key.code {
                KeyCode::Esc => clear_plugin_state(app),
                KeyCode::Backspace => {
                    app.plugin_input.pop();
                }
                KeyCode::Char(value)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    app.plugin_input.push(value);
                }
                KeyCode::Enter => {
                    let input = app.plugin_input.trim().to_owned();
                    if input.is_empty() {
                        app.message = "plugin input is required".into();
                        continue;
                    }
                    let Some(action_id) = app.plugin_action.clone() else {
                        clear_plugin_state(app);
                        continue;
                    };
                    let plugin = app
                        .behavior
                        .action(None, &action_id)
                        .map(|action| action.plugin.clone());
                    let Ok(Some(ActionPlugin::Steel {
                        path,
                        entrypoint,
                        capabilities,
                        ..
                    })) = plugin
                    else {
                        app.message = "Steel plugin action is unavailable".into();
                        clear_plugin_state(app);
                        continue;
                    };
                    let source = session.config_source(&path).await?;
                    suspend_tui(terminal)?;
                    let result =
                        execute_steel_plugin(source, entrypoint, input, capabilities).await;
                    resume_tui(terminal)?;
                    match result {
                        Ok(result) if result.choices.is_empty() => {
                            app.message = "plugin returned no results".into();
                            clear_plugin_state(app);
                        }
                        Ok(result) => {
                            app.plugin_results = result.choices;
                            app.plugin_index = 0;
                            app.mode = Mode::PluginResults;
                        }
                        Err(error) => {
                            app.message = format!("Steel plugin failed: {error:#}");
                            clear_plugin_state(app);
                        }
                    }
                }
                _ => {}
            },
            Mode::PluginResults => match key.code {
                KeyCode::Esc => clear_plugin_state(app),
                KeyCode::Up => {
                    app.plugin_index = app.plugin_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    app.plugin_index =
                        (app.plugin_index + 1).min(app.plugin_results.len().saturating_sub(1));
                }
                KeyCode::Char(value @ '1'..='9') => {
                    let index = usize::try_from(value.to_digit(10).unwrap_or_default())
                        .unwrap_or_default()
                        .saturating_sub(1);
                    add_plugin_result(app, session, index).await?;
                }
                KeyCode::Enter => {
                    add_plugin_result(app, session, app.plugin_index).await?;
                }
                _ => {}
            },
            Mode::MobilePairing => match key.code {
                KeyCode::Esc | KeyCode::Enter => app.cancel_mobile_pairing(),
                KeyCode::Char('c') => {
                    if let Some(pairing) = &app.mobile_pairing {
                        let link = pairing.setup_url.clone();
                        match copy_to_clipboard(terminal, &link) {
                            Ok(()) => app.message = "mobile setup link copied".into(),
                            Err(error) => {
                                app.message = format!("could not copy setup link: {error}");
                            }
                        }
                    }
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
                        if let Some(invitation) = app.pairing_invitation() {
                            match copy_to_clipboard(terminal, &invitation) {
                                Ok(()) => app.message = "writable ticket copied".into(),
                                Err(error) => {
                                    if let Some(pairing) = &mut app.pairing {
                                        pairing.error = format!("could not copy ticket: {error}");
                                    }
                                }
                            }
                        }
                    }
                    (Some(PairingStep::ServerCommand), KeyCode::Char('C')) => {
                        if let Some(command) = app.pairing_command() {
                            match copy_to_clipboard(terminal, &command) {
                                Ok(()) => app.message = "CLI fallback copied".into(),
                                Err(error) => {
                                    if let Some(pairing) = &mut app.pairing {
                                        pairing.error =
                                            format!("could not copy CLI fallback: {error}");
                                    }
                                }
                            }
                        }
                    }
                    (Some(PairingStep::ServerCommand), KeyCode::Char('U')) => {
                        if let Some(command) = app.user_syncd_command() {
                            match copy_to_clipboard(terminal, &command) {
                                Ok(()) => app.message = "user-unit installer command copied".into(),
                                Err(error) => {
                                    if let Some(pairing) = &mut app.pairing {
                                        pairing.error =
                                            format!("could not copy installer command: {error}");
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
                                pairing.error =
                                    "paste the server ticket returned by the setup page".into();
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
                KeyCode::Char(value) if value == app.leader_key => {
                    app.mode = Mode::Leader;
                }
                KeyCode::Char('q') => break,
                KeyCode::Tab => {
                    if !app.cycle_subview(true) {
                        app.next_pane();
                    }
                }
                KeyCode::BackTab => {
                    if !app.cycle_subview(false) {
                        app.previous_pane();
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => app.focus_left(),
                KeyCode::Right | KeyCode::Char('l') => app.focus_right(),
                KeyCode::Down | KeyCode::Char('j') => match app.pane {
                    app::Pane::Tags => app.select_next_tag(),
                    _ => app.select_next(),
                },
                KeyCode::Up | KeyCode::Char('k') => match app.pane {
                    app::Pane::Tags => app.select_previous_tag(),
                    _ => app.select_previous(),
                },
                KeyCode::Enter if app.pane == app::Pane::Tags => {
                    app.toggle_highlighted_tag();
                }
                KeyCode::Char('/') => {
                    app.mode = Mode::Search;
                }
                KeyCode::Char('g') => {
                    app.goto_index = app
                        .main_view_choices()
                        .iter()
                        .position(|choice| choice.view == app.active_view)
                        .unwrap_or(0);
                    app.mode = Mode::ViewPicker;
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
                KeyCode::Char('C') => {
                    app.create_title.clear();
                    app.mode = Mode::CreateEncryptedTitle;
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

fn clear_plugin_state(app: &mut App) {
    app.plugin_input.clear();
    app.plugin_action = None;
    app.plugin_prompt.clear();
    app.plugin_results.clear();
    app.plugin_index = 0;
    app.mode = Mode::Normal;
}

async fn add_plugin_result(
    app: &mut App,
    session: &mut WorkspaceSession,
    index: usize,
) -> Result<()> {
    let Some(PluginChoice { mut note, label }) = app.plugin_results.get(index).cloned() else {
        app.message = "plugin result is unavailable".into();
        return Ok(());
    };
    let now = xo_core::timestamp::now_local()?;
    let id = NoteId::new(xo_core::id::generate(now));
    let created = xo_core::timestamp::format(now)?;
    note.frontmatter
        .insert("id".into(), FrontmatterValue::String(id.to_string()));
    note.frontmatter
        .insert("created".into(), FrontmatterValue::String(created));
    let saved = Note {
        path: xo_core::projection::canonical_note_path(&id, &note.frontmatter),
        id,
        frontmatter: note.frontmatter,
        body: note.body,
    };
    session.save(&saved).await?;
    app.search.clear();
    app.selected_tags.clear();
    app.set_view("all");
    app.add_note(saved);
    clear_plugin_state(app);
    app.message = format!("added {label}");
    Ok(())
}

async fn capture_url(session: &mut WorkspaceSession, raw_url: &str) -> Result<Note> {
    let page = UrlCaptureService::default().capture(raw_url).await?;
    let note = captured_note(page, xo_core::timestamp::now_local()?)?;
    session.save(&note).await?;
    Ok(note)
}

async fn edit_workspace_config(session: &mut WorkspaceSession) -> Result<()> {
    let source = session.workspace_config_source().await?;
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let edited = external_edit_with(&editor, &[], source.as_bytes())?;
    let source = String::from_utf8(edited).context("workspace configuration is not UTF-8")?;
    session.save_workspace_config(&source).await
}

async fn create_note(app: &mut App, session: &mut WorkspaceSession, title: &str) -> Result<()> {
    let instant = xo_core::timestamp::now_local()?;
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

async fn create_encrypted_note(
    app: &mut App,
    session: &mut WorkspaceSession,
    title: &str,
) -> Result<()> {
    let passphrase = new_encryption_passphrase()?;
    let instant = xo_core::timestamp::now_local()?;
    let editor = std::env::var_os("EDITOR").unwrap_or_else(|| "vi".into());
    let note = prepare_encrypted_note(instant, title, &passphrase, &editor, &[])?;
    session.save(&note).await?;
    app.search.clear();
    app.selected_tags.clear();
    app.set_view("all");
    app.add_note(note.clone());
    app.message = format!("created encrypted {}", note.id);
    Ok(())
}

fn prepare_encrypted_note(
    instant: OffsetDateTime,
    title: &str,
    passphrase: &str,
    editor: &std::ffi::OsStr,
    editor_args: &[&std::ffi::OsStr],
) -> Result<Note> {
    let mut note = new_note_draft(instant, title)?;
    let document = Zeroizing::new(xo_core::markdown::render(&note.frontmatter, &note.body)?);
    let edited = Zeroizing::new(external_edit_with(
        editor,
        editor_args,
        document.as_bytes(),
    )?);
    let parsed = xo_core::markdown::parse(std::str::from_utf8(&edited)?)?;
    let created = match note.frontmatter.get("created") {
        Some(FrontmatterValue::String(value)) => value.clone(),
        _ => unreachable!("new encrypted note drafts always have a creation timestamp"),
    };
    note.frontmatter = required_frontmatter(
        parsed.frontmatter.unwrap_or_default(),
        note.id.as_str(),
        &created,
    );
    let plaintext = Zeroizing::new(parsed.body);
    note.body = xo_core::encryption::encrypt(note.id.as_str(), passphrase, &plaintext)?;
    note.path = xo_core::projection::canonical_note_path(&note.id, &note.frontmatter);
    Ok(note)
}

fn new_note_draft(instant: OffsetDateTime, title: &str) -> Result<Note> {
    let created = xo_core::timestamp::format(instant)?;
    let id = xo_core::id::generate(instant);
    let mut frontmatter = Frontmatter::new();
    frontmatter.insert(
        "title".into(),
        xo_core::domain::FrontmatterValue::String(title.into()),
    );
    let id = NoteId::new(id);
    let frontmatter = required_frontmatter(frontmatter, id.as_str(), &created);
    Ok(Note {
        path: xo_core::projection::canonical_note_path(&id, &frontmatter),
        id,
        frontmatter,
        body: String::new(),
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
        anyhow::ensure!(
            !xo_core::encryption::is_encrypted(&parsed.body),
            "existing plaintext notes cannot be converted to encrypted notes because their history remains plaintext"
        );
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

fn new_encryption_passphrase() -> Result<Zeroizing<String>> {
    let passphrase = password("New passphrase: ")?;
    anyhow::ensure!(!passphrase.is_empty(), "passphrase must not be empty");
    let confirmation = password("Confirm passphrase: ")?;
    anyhow::ensure!(
        passphrase.as_str() == confirmation.as_str(),
        "passphrases do not match"
    );
    Ok(passphrase)
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
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn command_version_matches_the_embedded_git_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(xo_core::version::VERSION)
        );
    }

    #[test]
    fn leader_keys_route_to_the_documented_commands() {
        assert_eq!(leader_command('t'), Some(LeaderCommand::ToggleTags));
        assert_eq!(leader_command('v'), Some(LeaderCommand::ChooseView));
        assert_eq!(leader_command('a'), Some(LeaderCommand::Actions));
        assert_eq!(leader_command('c'), Some(LeaderCommand::Config));
        assert_eq!(leader_command('m'), Some(LeaderCommand::Mobile));
        assert_eq!(leader_command('j'), Some(LeaderCommand::Server));
        assert_eq!(leader_command('s'), Some(LeaderCommand::Sync));
        assert_eq!(leader_command('?'), None);
    }

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
    fn new_encrypted_note_never_creates_a_plaintext_revision() {
        let instant = OffsetDateTime::from_unix_timestamp(1_750_000_000).unwrap();
        let args = [
            std::ffi::OsStr::new("-c"),
            std::ffi::OsStr::new(
                "grep -q '^title: Secret title$' \"$1\" || exit 8; \
                 printf '%s' '---\nid: changed\ntitle: Edited secret\ntype: note\ntags: [private]\n---\nprivate body' > \"$1\"",
            ),
            std::ffi::OsStr::new("_"),
        ];
        let note = prepare_encrypted_note(
            instant,
            "Secret title",
            "passphrase",
            std::ffi::OsStr::new("sh"),
            &args,
        )
        .unwrap();
        assert!(xo_core::encryption::is_encrypted(&note.body));
        assert_eq!(
            xo_core::encryption::decrypt(note.id.as_str(), "passphrase", &note.body).unwrap(),
            "private body"
        );
        assert_eq!(
            note.frontmatter.get("id"),
            Some(&FrontmatterValue::String(note.id.to_string()))
        );
        assert_eq!(
            note.frontmatter.get("title"),
            Some(&FrontmatterValue::String("Edited secret".into()))
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
        let prefix = &note.id.as_str()[..3];
        assert_eq!(
            note.path,
            format!("{prefix}/{}-my-title.md", note.id.as_str())
        );
        assert!(note.body.is_empty());
    }
}
