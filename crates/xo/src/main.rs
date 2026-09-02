mod app;

use std::io::{self, IsTerminal as _, Write as _, stdout};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use app::{App, Mode, external_edit_with, external_edit_with_suffix, render, required_frontmatter};
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
use xo::session::{WorkspaceEvent, WorkspaceSession};
use xo::steel_plugin::{
    PluginChoice, PluginContext, PluginItem, PluginOperation,
    execute_with_context as execute_steel_plugin_with_context,
};
use xo::url_capture::{UrlCaptureService, captured_note};
use xo_core::behavior::ActionPlugin;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::{Note, NoteId};
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(
    name = "xo",
    version = xo_core::version::VERSION,
    about = "Offline-first personal knowledge workspace"
)]
struct Cli {
    /// Override the durable local replica directory from config.scm.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Override the human-readable presence client ID (defaults to the host name).
    #[arg(long)]
    client_id: Option<String>,
    /// Central xo-syncd HTTP(S) base URL.
    #[arg(long, default_value = "http://127.0.0.1:9464")]
    server: String,
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
    /// Print the default ~/.config/xo/keys.scm keymap to stdout.
    KeymapInit,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    match &cli.command {
        Some(Command::ConfigInit) => print!("{}", XoConfig::default().document()?),
        Some(Command::KeymapInit) => print!("{}", xo::keymap::DEFAULT_KEYS),
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
            import_command(&cli, source, item_type).await?;
        }
        Some(Command::Export {
            destination,
            item_type,
        }) => {
            let config = configured(&cli)?;
            let access_token = xo::oauth::access_token(&cli.server, &home_dir()?).await?;
            let session = WorkspaceSession::open_central_with_plugins(
                &config.state_dir,
                &cli.server,
                config.projection.clone(),
                config.resolved_client_id()?,
                access_token,
                local_plugins()?,
            )
            .await?;
            let result =
                xo::content_io::export_markdown(&session, destination, item_type.as_deref()).await;
            let shutdown = session.shutdown().await;
            let exported = result?;
            shutdown?;
            println!("exported={}", exported.exported);
        }
        None => {
            let config = configured(&cli)?;
            run_tui(
                &config.state_dir,
                &cli.server,
                config.projection.clone(),
                &config_path(&home_dir()?).with_file_name("keys.scm"),
                config.resolved_client_id()?,
            )
            .await?;
        }
    }
    Ok(())
}

async fn import_command(cli: &Cli, source: &std::path::Path, item_type: &str) -> Result<()> {
    let config = configured(cli)?;
    let access_token = xo::oauth::access_token(&cli.server, &home_dir()?).await?;
    let mut session = WorkspaceSession::open_central_with_plugins(
        &config.state_dir,
        &cli.server,
        config.projection.clone(),
        config.resolved_client_id()?,
        access_token,
        local_plugins()?,
    )
    .await?;
    let interactive = io::stderr().is_terminal();
    let mut progress_line = false;
    eprintln!("Scanning {} for Markdown items...", source.display());
    let result = xo::content_io::import_markdown_with_progress(
        &mut session,
        source,
        item_type,
        |progress| match progress {
            xo::content_io::ImportProgress::Found { total } => {
                eprintln!("Found {total} item(s) ready to import.");
            }
            xo::content_io::ImportProgress::Processed { current, total } => {
                if interactive {
                    eprint!("\rImporting items: {current}/{total}");
                    let _ = io::stderr().flush();
                    progress_line = true;
                } else {
                    eprintln!("Importing item {current}/{total}");
                }
            }
            xo::content_io::ImportProgress::Finalizing => {
                if progress_line {
                    eprintln!();
                    progress_line = false;
                }
                eprintln!("Finalizing projection and durable local replica...");
            }
        },
    )
    .await;
    if progress_line {
        eprintln!();
    }
    let shutdown = session.shutdown().await;
    let imported = result?;
    shutdown?;
    eprintln!("Import finalized and all local stores closed cleanly.");
    println!("imported={imported}");
    Ok(())
}

fn plugin_context(app: &App) -> PluginContext {
    let selected_item_ids = app
        .selected_note_ids()
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    let items = app
        .notes
        .iter()
        .map(|note| {
            (
                note.id.to_string(),
                PluginItem {
                    frontmatter: note.frontmatter.clone(),
                    body: note.body.clone(),
                },
            )
        })
        .collect();
    let all_tags = app.all_note_tags();
    PluginContext {
        selected_item_ids,
        items,
        all_tags,
    }
}

fn local_plugins() -> Result<std::collections::BTreeMap<String, String>> {
    let directory = config_path(&home_dir()?)
        .parent()
        .context("xo config path has no parent")?
        .join("plugins");
    let mut plugins = std::collections::BTreeMap::new();
    if !directory.exists() {
        return Ok(plugins);
    }
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("scm")
        {
            continue;
        }
        let name = path
            .file_name()
            .context("plugin has no file name")?
            .to_string_lossy();
        let logical_path = format!("plugins/{name}");
        if !xo_core::steel_runtime::valid_plugin_path(&logical_path) {
            continue;
        }
        plugins.insert(logical_path, std::fs::read_to_string(path)?);
    }
    Ok(plugins)
}

fn configured(cli: &Cli) -> Result<XoConfig> {
    let home = home_dir()?;
    let config = XoConfig::load(&config_path(&home), &home)?.apply(
        CliOverrides {
            state_dir: cli.state_dir.clone(),
            client_id: cli.client_id.clone(),
            projection: cli.projection.clone(),
        },
        &home,
    );
    config.resolved_client_id()?;
    Ok(config)
}

async fn run_tui(
    state_dir: &std::path::Path,
    server: &str,
    projection: PathBuf,
    keys_path: &std::path::Path,
    client_id: xo_core::ClientId,
) -> Result<()> {
    let access_token = xo::oauth::access_token(server, &home_dir()?).await?;
    let mut session = WorkspaceSession::open_central_with_plugins(
        state_dir,
        server,
        projection,
        client_id,
        access_token,
        local_plugins()?,
    )
    .await?;
    let workspace_events = session.subscribe()?;
    let behavior = session.behavior().await?;
    let snapshot = session.snapshot().await?;
    let mut app = App::new(behavior, snapshot.notes.clone());
    app.workspace_id = session.workspace_id();
    let (keymap, keys_source) = xo::keymap::KeyMap::load_or_create(keys_path)?;
    app.keymap = keymap;
    hydrate(&mut app, &session, snapshot).await?;
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(
        &mut terminal,
        &mut app,
        &mut session,
        workspace_events,
        keys_path,
        keys_source,
    )
    .await;
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
    app.connected_clients = session.connected_clients().await;
    session.client_id().clone_into(&mut app.client_id);
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

#[allow(clippy::too_many_lines)]
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &mut WorkspaceSession,
    mut workspace_events: impl Stream<Item = Result<WorkspaceEvent>> + Unpin,
    keys_path: &std::path::Path,
    mut keys_source: String,
) -> Result<()> {
    let mut terminal_events = EventStream::new();
    let mut keymap_reload = tokio::time::interval(std::time::Duration::from_millis(500));
    keymap_reload.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        terminal.draw(|frame| render(frame, app))?;
        let event = tokio::select! {
            terminal_event = terminal_events.next() => terminal_event
                .context("terminal event stream ended")?
                .context("read terminal event")?,
            _ = keymap_reload.tick() => {
                match std::fs::read_to_string(keys_path) {
                    Ok(source) if source != keys_source => match xo::keymap::KeyMap::from_source(&source) {
                        Ok(keymap) => {
                            app.keymap = keymap;
                            keys_source = source;
                            app.message = "key bindings reloaded".into();
                        }
                        Err(error) => app.message = format!("keys.scm reload failed: {error:#}"),
                    },
                    Ok(_) => {}
                    Err(error) => app.message = format!("keys.scm reload failed: {error}"),
                }
                continue;
            }
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
                KeyCode::Esc => {
                    let action = app.keymap.action_for(key).cloned();
                    if let Some(action) = action
                        && dispatch_action(terminal, app, session, &action).await?
                    {
                        break;
                    }
                }
                KeyCode::Enter => app.mode = Mode::Normal,
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
                    app.action_index = 0;
                }
                KeyCode::Up => app.action_index = app.action_index.saturating_sub(1),
                KeyCode::Down => {
                    app.action_index = (app.action_index + 1)
                        .min(app.matching_tui_actions().len().saturating_sub(1));
                }
                KeyCode::Tab => {
                    if let Some(action) = app.matching_tui_actions().get(app.action_index) {
                        app.action_query.clone_from(action);
                    }
                }
                KeyCode::Enter => {
                    let command = if app.action_query.trim().is_empty() {
                        app.matching_tui_actions().get(app.action_index).cloned()
                    } else {
                        Some(app.action_query.clone())
                    };
                    app.mode = Mode::Normal;
                    if let Some(command) = command {
                        match xo::keymap::ActionCall::parse(&command) {
                            Ok(action)
                                if dispatch_action(terminal, app, session, &action).await? =>
                            {
                                break;
                            }
                            Ok(_) => {}
                            Err(error) => app.message = format!("invalid action: {error:#}"),
                        }
                    }
                }
                KeyCode::Char(value)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    app.action_query.push(value);
                    app.action_index = 0;
                }
                _ => {}
            },
            Mode::ItemActionPicker => match key.code {
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
                            Ok(Some(ActionPlugin::TagPicker)) => {
                                if app.selected_note_ids().is_empty() {
                                    app.message = "select at least one note first".into();
                                    continue;
                                }
                                app.selected_tags_for_picker();
                                app.mode = Mode::TagPicker;
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
            Mode::TagPicker => match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Up => app.tag_picker_index = app.tag_picker_index.saturating_sub(1),
                KeyCode::Down => {
                    app.tag_picker_index = (app.tag_picker_index + 1)
                        .min(app.tag_picker_choices().len().saturating_sub(1));
                }
                KeyCode::Char(' ') => app.toggle_tag_picker_tag(),
                KeyCode::Enter => {
                    let notes = app.apply_tag_picker();
                    for note in notes {
                        session.save(&note).await?;
                    }
                    app.clear_selected_notes();
                    app.mode = Mode::Normal;
                    app.message = "updated tags".into();
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
                    let source = session
                        .config_source(&path)
                        .await
                        .with_context(|| format!("load Steel plugin {path}"));
                    let result = match source {
                        Ok(source) => {
                            suspend_tui(terminal)?;
                            let context = plugin_context(app);
                            let result = execute_steel_plugin_with_context(
                                source,
                                entrypoint,
                                input,
                                capabilities,
                                context,
                            )
                            .await;
                            resume_tui(terminal)?;
                            result
                        }
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(result) => {
                            apply_plugin_operations(app, session, &result.operations).await?;
                            if result.choices.is_empty() {
                                app.message = "plugin completed".into();
                                clear_plugin_state(app);
                                continue;
                            }
                            app.plugin_results = result.choices;
                            app.plugin_index = 0;
                            app.mode = Mode::PluginResults;
                        }
                        Err(error) => {
                            app.message = format!("Notice: Hardcover search failed: {error:#}");
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
            Mode::Clients if key.code == KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.message.clear();
            }
            Mode::Clients => {
                let action = app.keymap.action_for(key).cloned();
                if let Some(action) = action {
                    dispatch_action(terminal, app, session, &action).await?;
                }
            }
            _ => {
                let action = app.keymap.action_for(key).cloned();
                if let Some(action) = action
                    && dispatch_action(terminal, app, session, &action).await?
                {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn dispatch_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &mut WorkspaceSession,
    action: &xo::keymap::ActionCall,
) -> Result<bool> {
    match action.name.as_str() {
        "quit" => return Ok(true),
        "cursor_down" if app.mode == Mode::Clients => {
            let count = app.connected_clients.len();
            if count > 0 {
                app.selected = (app.selected + 1).min(count - 1);
            }
        }
        "cursor_down" => match app.pane {
            app::Pane::Tags => app.select_next_tag(),
            _ => app.select_next(),
        },
        "cursor_up" if app.mode == Mode::Clients => {
            app.selected = app.selected.saturating_sub(1);
        }
        "cursor_up" => match app.pane {
            app::Pane::Tags => app.select_previous_tag(),
            _ => app.select_previous(),
        },
        "focus_column_left" => app.focus_left(),
        "focus_column_right" => app.focus_right(),
        "focus_subview_next" => {
            app.cycle_subview(true);
        }
        "focus_subview_previous" => {
            app.cycle_subview(false);
        }
        "toggle_selection" => app.toggle_selected_note(),
        "toggle_tag" if app.pane == app::Pane::Tags => app.toggle_highlighted_tag(),
        "toggle_tag" => {}
        "open_search" => app.mode = Mode::Search,
        "clear_search" => app.clear_search(),
        "open_goto" => {
            app.goto_input.clear();
            app.goto_index = 0;
            app.mode = Mode::Goto;
        }
        "open_view_picker" => {
            app.goto_index = app
                .main_view_choices()
                .iter()
                .position(|choice| choice.view == app.active_view)
                .unwrap_or(0);
            app.mode = Mode::ViewPicker;
        }
        "action_picker" => {
            app.action_query.clear();
            app.action_index = 0;
            app.mode = Mode::ActionPicker;
        }
        "open_item_actions" => {
            app.action_query.clear();
            app.mode = Mode::ItemActionPicker;
        }
        "goto_view" => {
            app.goto_view_path(&action.arguments[0]);
        }
        "create_item" => {
            app.create_title.clear();
            app.mode = Mode::CreateTitle;
        }
        "create_encrypted_item" => {
            app.create_title.clear();
            app.mode = Mode::CreateEncryptedTitle;
        }
        "edit_item" => {
            suspend_tui(terminal)?;
            let result = edit_note(app, session).await;
            resume_tui(terminal)?;
            result?;
        }
        "delete_item" => {
            if let Some(note) = app.selected_note().cloned() {
                session.delete(&note).await?;
                app.delete_selected();
                app.message = format!("deleted {}", note.id);
            }
        }
        "restore_item" => {
            if let Some(id) = app.deleted.keys().next_back().cloned()
                && let Some(note) = app.restore(&id)
            {
                session.save(&note).await?;
                app.message = format!("restored {id}");
            }
        }
        "retry_operation" => {
            if let Some(operation) = app.operations.first() {
                session.retry(operation.id)?;
                app.message = format!("queued retry {}", operation.id);
            }
        }
        "toggle_tags_column" => app.toggle_tags_visible(),
        "reverse_sort" => app.toggle_sort(),
        "refresh_sync" => {
            session.refresh_sync()?;
            refresh_workspace(app, session).await?;
            app.message = "refreshed and retried synchronization".into();
        }
        "edit_workspace_config" => {
            suspend_tui(terminal)?;
            let result = edit_workspace_config(session).await;
            resume_tui(terminal)?;
            result?;
            refresh_workspace(app, session).await?;
            app.message = "workspace configuration updated".into();
        }
        "open_sync_status" => {
            app.mode = Mode::Sync;
            app.message = sync_summary(app);
        }
        "open_conflicts" => {
            app.mode = Mode::Conflicts;
            app.message = conflict_summary(app);
        }
        "open_peers" => {
            app.selected = 0;
            app.mode = Mode::Clients;
            app.message = device_summary(app);
        }
        "unlock_preview" => {
            suspend_tui(terminal)?;
            let result = unlock(app);
            resume_tui(terminal)?;
            result?;
        }
        _ => {
            let plugin = app
                .behavior
                .action(app.selected_note(), &action.name)
                .map(|descriptor| descriptor.plugin.clone());
            match plugin {
                Ok(Some(ActionPlugin::TagPicker)) => {
                    app.selected_tags_for_picker();
                    app.mode = Mode::TagPicker;
                }
                Ok(Some(ActionPlugin::Steel { prompt, .. })) => {
                    app.plugin_input.clear();
                    app.plugin_results.clear();
                    app.plugin_index = 0;
                    app.plugin_action = Some(action.name.clone());
                    app.plugin_prompt = prompt;
                    app.mode = Mode::PluginInput;
                }
                Ok(None) => {
                    let note = app.run_action(&action.name)?;
                    session.save(&note).await?;
                    app.message = format!("applied {}", action.name);
                }
                Ok(Some(ActionPlugin::CaptureUrl)) => {
                    app.capture_url.clear();
                    app.mode = Mode::CaptureUrl;
                }
                Err(error) => app.message = error.to_string(),
            }
        }
    }
    Ok(false)
}

fn clear_plugin_state(app: &mut App) {
    app.plugin_input.clear();
    app.plugin_action = None;
    app.plugin_prompt.clear();
    app.plugin_results.clear();
    app.plugin_index = 0;
    app.mode = Mode::Normal;
}

async fn apply_plugin_operations(
    app: &mut App,
    session: &mut WorkspaceSession,
    operations: &[PluginOperation],
) -> Result<()> {
    for operation in operations {
        match operation {
            PluginOperation::UpdateItem {
                id,
                frontmatter,
                body,
            } => {
                let note_id = NoteId::new(id.clone());
                let Some(note) = app.notes.iter_mut().find(|note| note.id == note_id) else {
                    bail!("plugin update targets unavailable item {id}");
                };
                note.frontmatter = frontmatter.clone();
                note.body.clone_from(body);
                note.path = xo_core::projection::canonical_note_path(&note.id, &note.frontmatter);
                let note = note.clone();
                session.save(&note).await?;
            }
            PluginOperation::CreateItem { frontmatter, body } => {
                let now = xo_core::timestamp::now_local()?;
                let id = NoteId::new(xo_core::id::generate(now));
                let created = xo_core::timestamp::format(now)?;
                let frontmatter = required_frontmatter(frontmatter.clone(), id.as_str(), &created);
                let note = Note {
                    path: xo_core::projection::canonical_note_path(&id, &frontmatter),
                    id,
                    frontmatter,
                    body: body.clone(),
                };
                session.save(&note).await?;
                app.add_note(note);
            }
        }
    }
    Ok(())
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
    let edited = external_edit_with_suffix(&editor, &[], source.as_bytes(), ".xo.scm")?;
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
    if app.connected_clients.is_empty() {
        "no clients connected".to_owned()
    } else {
        app.connected_clients.join(" | ")
    }
}
fn sync_summary(app: &App) -> String {
    let server = app
        .sync
        .as_ref()
        .map_or("offline", |status| match status.connectivity {
            xo_core::sync_state::Connectivity::Connected => "connected",
            xo_core::sync_state::Connectivity::Connecting => "connecting",
            xo_core::sync_state::Connectivity::Offline => "offline",
        });
    format!(
        "server: {server}; operations: {}; missing blobs: {}",
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
            "--server",
            "https://xo.example.test",
            "--projection",
            "/notes",
        ])
        .unwrap();
        assert_eq!(
            cli.state_dir.as_deref(),
            Some(std::path::Path::new("/state"))
        );
        assert_eq!(cli.server, "https://xo.example.test");
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
