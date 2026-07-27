use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write;
use std::process::Command;

use anyhow::{Context, Result, bail};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tempfile::NamedTempFile;
use xo_core::behavior::{Query, WorkspaceBehavior};
use xo_core::domain::{DeviceRecord, Frontmatter, FrontmatterValue};
use xo_core::encryption;
use xo_core::projection::Diagnostic;
use xo_core::sync_state::{DurableOperation, SyncStatus};
use xo_core::{Conflict, Note, NoteId, NoteRevision, RevisionId};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pane {
    Tags,
    Notes,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Normal,
    Search,
    CreateTitle,
    Goto,
    ActionPicker,
    Conflicts,
    Devices,
    Sync,
    Pairing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingStep {
    StateDirectory,
    ServerCommand,
    ServerOutput,
    Connected,
}

pub struct ServerPairing {
    pub step: PairingStep,
    pub state_dir: String,
    pub invitation: Option<Zeroizing<String>>,
    pub server_output: Zeroizing<String>,
    pub reveal_ticket: bool,
    pub error: String,
}

pub struct App {
    pub workspace_id: String,
    pub behavior: WorkspaceBehavior,
    pub notes: Vec<Note>,
    pub deleted: BTreeMap<NoteId, Note>,
    pub conflicts: Vec<Conflict>,
    pub conflict_history: BTreeMap<NoteId, Vec<(RevisionId, NoteRevision)>>,
    pub devices: Vec<DeviceRecord>,
    pub diagnostics: Vec<Diagnostic>,
    pub operations: Vec<DurableOperation>,
    pub sync: Option<SyncStatus>,
    pub active_view: String,
    pub active_subview: Option<String>,
    pub search: String,
    pub selected_tags: BTreeSet<String>,
    pub tags_visible: bool,
    pub pane: Pane,
    pub mode: Mode,
    pub selected: usize,
    pub tag_index: usize,
    pub action_query: String,
    pub create_title: String,
    pub goto_input: String,
    pub goto_index: usize,
    pub message: String,
    pub pairing: Option<ServerPairing>,
    pub decrypted_preview: Option<Zeroizing<String>>,
    sort_descending: bool,
}

impl App {
    pub fn new(behavior: WorkspaceBehavior, notes: Vec<Note>) -> Self {
        let active_view = behavior.default_view.clone();
        let tags_visible = behavior
            .views
            .iter()
            .find(|view| view.id == active_view)
            .is_none_or(|view| view.show_tags);
        Self {
            workspace_id: String::new(),
            behavior,
            notes,
            deleted: BTreeMap::new(),
            conflicts: vec![],
            conflict_history: BTreeMap::new(),
            devices: vec![],
            diagnostics: vec![],
            operations: vec![],
            sync: None,
            active_view,
            active_subview: None,
            search: String::new(),
            selected_tags: BTreeSet::new(),
            tags_visible,
            pane: Pane::Notes,
            mode: Mode::Normal,
            selected: 0,
            tag_index: 0,
            action_query: String::new(),
            create_title: String::new(),
            goto_input: String::new(),
            goto_index: 0,
            message: String::new(),
            pairing: None,
            decrypted_preview: None,
            sort_descending: false,
        }
    }

    pub fn visible_notes(&self) -> Vec<&Note> {
        let mut notes = self.query_notes(self.selected_tags.clone());
        if self.sort_descending {
            notes.reverse();
        }
        notes
    }

    fn query_notes(&self, tags: BTreeSet<String>) -> Vec<&Note> {
        self.behavior
            .query(
                &self.notes,
                &Query {
                    view: self.active_view.clone(),
                    subview: self.active_subview.clone(),
                    title: (!self.search.is_empty()).then(|| self.search.clone()),
                    tags,
                    limit: None,
                },
            )
            .unwrap_or_default()
    }

    pub fn selected_note(&self) -> Option<&Note> {
        self.visible_notes().get(self.selected_index()?).copied()
    }
    pub fn selected_index(&self) -> Option<usize> {
        let len = self.visible_notes().len();
        (len > 0).then(|| self.selected.min(len - 1))
    }
    pub fn next_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Notes => Pane::Preview,
            Pane::Preview if self.tags_visible => Pane::Tags,
            Pane::Tags | Pane::Preview => Pane::Notes,
        };
    }
    pub fn previous_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Notes if self.tags_visible => Pane::Tags,
            Pane::Preview => Pane::Notes,
            Pane::Tags | Pane::Notes => Pane::Preview,
        };
    }
    pub fn toggle_tags_visible(&mut self) {
        self.tags_visible = !self.tags_visible;
        if !self.tags_visible && self.pane == Pane::Tags {
            self.pane = Pane::Notes;
        }
    }
    pub fn select_next(&mut self) {
        let len = self.visible_notes().len();
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
            self.decrypted_preview = None;
        }
    }
    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.decrypted_preview = None;
    }
    pub fn available_tags(&self) -> Vec<(String, usize)> {
        let mut tags = BTreeSet::new();
        for note in self.query_notes(BTreeSet::new()) {
            for tag in note_tags(note) {
                tags.insert(tag);
            }
        }
        tags.extend(self.selected_tags.iter().cloned());
        tags.into_iter()
            .map(|tag| {
                let mut filters = self.selected_tags.clone();
                filters.insert(tag.clone());
                let count = self.query_notes(filters).len();
                (tag, count)
            })
            .collect()
    }
    pub fn select_next_tag(&mut self) {
        let last = self.available_tags().len().saturating_sub(1);
        self.tag_index = (self.tag_index + 1).min(last);
    }
    pub fn select_previous_tag(&mut self) {
        self.tag_index = self.tag_index.saturating_sub(1);
    }
    pub fn toggle_highlighted_tag(&mut self) {
        if let Some((tag, _)) = self.available_tags().get(self.tag_index).cloned() {
            self.toggle_tag(&tag);
        }
    }
    pub fn set_view(&mut self, id: &str) {
        id.clone_into(&mut self.active_view);
        self.active_subview = None;
        self.selected = 0;
        self.tag_index = 0;
        self.tags_visible = self
            .behavior
            .views
            .iter()
            .find(|view| view.id == id)
            .is_none_or(|view| view.show_tags);
        if !self.tags_visible && self.pane == Pane::Tags {
            self.pane = Pane::Notes;
        }
    }
    pub fn set_subview(&mut self, id: Option<String>) {
        self.active_subview = id;
        self.selected = 0;
    }

    pub fn goto_choices(&self) -> Vec<ViewChoice> {
        let mut choices = self
            .behavior
            .views
            .iter()
            .flat_map(|view| {
                let mut choices = vec![ViewChoice {
                    label: view.name.clone(),
                    view: view.id.clone(),
                    subview: None,
                    navigation_key: view.id.to_lowercase(),
                    prefix: String::new(),
                }];
                choices.extend(view.subviews.iter().map(|subview| ViewChoice {
                    label: format!("{} / {}", view.name, subview.name),
                    view: view.id.clone(),
                    subview: Some(subview.id.clone()),
                    navigation_key: subview.id.to_lowercase(),
                    prefix: String::new(),
                }));
                choices
            })
            .collect::<Vec<_>>();
        for index in 0..choices.len() {
            let duplicate = choices.iter().enumerate().any(|(other_index, other)| {
                index != other_index && other.navigation_key == choices[index].navigation_key
            });
            if duplicate && choices[index].subview.is_some() {
                choices[index].navigation_key =
                    format!("{}/{}", choices[index].view, choices[index].navigation_key);
            }
        }
        let keys = choices
            .iter()
            .map(|choice| choice.navigation_key.clone())
            .collect::<Vec<_>>();
        for (index, choice) in choices.iter_mut().enumerate() {
            choice.prefix = shortest_unique_prefix(&keys, index);
        }
        let input = self.goto_input.to_lowercase();
        choices.retain(|choice| choice.prefix.starts_with(&input));
        choices
    }

    pub fn choose_goto(&mut self) -> bool {
        if let Some(choice) = self.goto_choices().get(self.goto_index).cloned() {
            self.set_view(&choice.view);
            self.set_subview(choice.subview);
            return true;
        }
        false
    }

    pub fn goto_is_unambiguous(&self) -> bool {
        !self.goto_input.is_empty() && self.goto_choices().len() == 1
    }
    pub fn toggle_tag(&mut self, tag: &str) {
        if !self.selected_tags.remove(tag) {
            self.selected_tags.insert(tag.to_owned());
        }
        self.selected = 0;
        self.tag_index = self
            .available_tags()
            .iter()
            .position(|(candidate, _)| candidate == tag)
            .unwrap_or(0);
    }
    pub fn toggle_sort(&mut self) {
        self.sort_descending = !self.sort_descending;
    }

    pub fn add_note(&mut self, note: Note) {
        let id = note.id.clone();
        self.notes.push(note);
        self.selected = self
            .visible_notes()
            .iter()
            .position(|note| note.id == id)
            .unwrap_or(0);
    }

    pub fn edit_selected(&mut self, body: String) -> Option<Note> {
        let id = self.selected_note()?.id.clone();
        let note = self.notes.iter_mut().find(|note| note.id == id)?;
        note.body = body;
        Some(note.clone())
    }

    pub fn replace_selected(&mut self, frontmatter: Frontmatter, body: String) -> Option<Note> {
        let id = self.selected_note()?.id.clone();
        let note = self.notes.iter_mut().find(|note| note.id == id)?;
        note.frontmatter = frontmatter;
        note.body = body;
        Some(note.clone())
    }

    pub fn delete_selected(&mut self) -> Option<Note> {
        let id = self.selected_note()?.id.clone();
        let index = self.notes.iter().position(|note| note.id == id)?;
        let note = self.notes.remove(index);
        self.deleted.insert(id, note.clone());
        self.selected = self.selected.saturating_sub(1);
        Some(note)
    }

    pub fn restore(&mut self, id: &NoteId) -> Option<Note> {
        let note = self.deleted.remove(id)?;
        self.notes.push(note.clone());
        Some(note)
    }

    pub fn matching_actions(&self) -> Vec<&xo_core::behavior::ActionDescriptor> {
        let Some(note) = self.selected_note() else {
            return vec![];
        };
        let needle = self.action_query.to_lowercase();
        let mut actions = self
            .behavior
            .actions
            .iter()
            .filter(|action| action.predicate.matches(note))
            .filter(|action| {
                fuzzy(&format!("{} {}", action.id, action.description), &needle).is_some()
            })
            .collect::<Vec<_>>();
        actions.sort_by_key(|action| {
            std::cmp::Reverse(
                fuzzy(&format!("{} {}", action.id, action.description), &needle)
                    .unwrap_or_default(),
            )
        });
        actions
    }

    pub fn run_action(&mut self, id: &str) -> Result<Note> {
        let note_id = self.selected_note().context("no selected note")?.id.clone();
        let note = self
            .notes
            .iter_mut()
            .find(|note| note.id == note_id)
            .context("selected note disappeared")?;
        self.behavior.apply_action(note, id)?;
        Ok(note.clone())
    }

    pub fn preview(&self, passphrase: Option<&str>) -> Result<String> {
        let note = self.selected_note().context("no selected note")?;
        let body = if encryption::is_encrypted(&note.body) {
            encryption::decrypt(
                note.id.as_str(),
                passphrase.context("encrypted note is locked")?,
                &note.body,
            )?
        } else {
            note.body.clone()
        };
        let mut visible = note.clone();
        visible.body = body;
        Ok(xo_core::markdown::render(
            &visible.frontmatter,
            &visible.body,
        )?)
    }

    pub fn unlock_preview(&mut self, passphrase: &str) -> Result<()> {
        self.decrypted_preview = Some(Zeroizing::new(self.preview(Some(passphrase))?));
        Ok(())
    }

    pub fn edit_encrypted_with(
        &mut self,
        passphrase: &str,
        program: &OsStr,
        args: &[&OsStr],
    ) -> Result<Note> {
        let note = self.selected_note().context("no selected note")?.clone();
        let plaintext = Zeroizing::new(encryption::decrypt(
            note.id.as_str(),
            passphrase,
            &note.body,
        )?);
        let edited = Zeroizing::new(external_edit_with(program, args, plaintext.as_bytes())?);
        let encrypted =
            encryption::encrypt(note.id.as_str(), passphrase, std::str::from_utf8(&edited)?)?;
        self.edit_selected(encrypted)
            .context("selected note disappeared")
    }

    pub fn start_server_pairing(&mut self) {
        self.pairing = Some(ServerPairing {
            step: PairingStep::StateDirectory,
            state_dir: "/var/lib/xo-syncd".into(),
            invitation: None,
            server_output: Zeroizing::new(String::new()),
            reveal_ticket: false,
            error: String::new(),
        });
        self.mode = Mode::Pairing;
    }

    pub fn cancel_server_pairing(&mut self) {
        self.pairing = None;
        self.mode = Mode::Normal;
    }

    pub fn set_pairing_invitation(&mut self, invitation: String) {
        if let Some(pairing) = &mut self.pairing {
            pairing.invitation = Some(Zeroizing::new(invitation));
            pairing.step = PairingStep::ServerCommand;
            pairing.error.clear();
        }
    }

    pub fn pairing_command(&self) -> Option<Zeroizing<String>> {
        let pairing = self.pairing.as_ref()?;
        let ticket = pairing.invitation.as_deref()?;
        Some(Zeroizing::new(server_pairing_commands(
            &pairing.state_dir,
            ticket,
        )))
    }

    pub fn pairing_ticket(&self) -> Option<Zeroizing<String>> {
        let pairing = self.pairing.as_ref()?;
        ticket_from_server_output(&pairing.server_output)
    }
}

#[must_use]
pub fn server_pairing_commands(state_dir: &str, ticket: &str) -> String {
    format!(
        "sudo systemctl stop xo-syncd\nsudo -u xo xo-admin import-ticket {} {}\nsudo systemctl start xo-syncd",
        shell_quote(state_dir),
        shell_quote(ticket)
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[must_use]
pub fn ticket_from_server_output(output: &str) -> Option<Zeroizing<String>> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("ticket="))
        .map(str::trim)
        .filter(|ticket| !ticket.is_empty())
        .map(|value| Zeroizing::new(value.to_owned()))
        .or_else(|| {
            let value = output.trim();
            (!value.is_empty() && !value.contains('=') && !value.chars().any(char::is_whitespace))
                .then(|| Zeroizing::new(value.to_owned()))
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewChoice {
    pub label: String,
    pub view: String,
    pub subview: Option<String>,
    pub navigation_key: String,
    pub prefix: String,
}

fn shortest_unique_prefix(keys: &[String], index: usize) -> String {
    let key = &keys[index];
    for length in 1..=key.chars().count() {
        let prefix = key.chars().take(length).collect::<String>();
        if keys
            .iter()
            .enumerate()
            .all(|(other_index, other)| other_index == index || !other.starts_with(&prefix))
        {
            return prefix;
        }
    }
    key.clone()
}

pub fn required_frontmatter(mut frontmatter: Frontmatter, id: &str, created: &str) -> Frontmatter {
    frontmatter.insert("id".into(), FrontmatterValue::String(id.into()));
    frontmatter.insert("created".into(), FrontmatterValue::String(created.into()));
    if !matches!(
        frontmatter.get("tags"),
        Some(FrontmatterValue::Sequence(_) | FrontmatterValue::String(_))
    ) {
        frontmatter.insert("tags".into(), FrontmatterValue::Sequence(vec![]));
    }
    if !matches!(frontmatter.get("title"), Some(FrontmatterValue::String(_))) {
        frontmatter.insert("title".into(), FrontmatterValue::String("Untitled".into()));
    }
    if !matches!(frontmatter.get("type"), Some(FrontmatterValue::String(_))) {
        frontmatter.insert("type".into(), FrontmatterValue::String("note".into()));
    }
    frontmatter
}

fn note_tags(note: &Note) -> Vec<String> {
    match note.frontmatter.get("tags") {
        Some(FrontmatterValue::Sequence(values)) => values
            .iter()
            .filter_map(|value| match value {
                FrontmatterValue::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        Some(FrontmatterValue::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => vec![],
    }
}

pub fn external_edit_with(program: &OsStr, args: &[&OsStr], initial: &[u8]) -> Result<Vec<u8>> {
    let mut file = NamedTempFile::new().context("create secure editor file")?;
    file.write_all(initial)?;
    file.flush()?;
    let status = Command::new(program)
        .args(args)
        .arg(file.path())
        .status()
        .context("start external editor")?;
    if !status.success() {
        bail!("external editor exited with {status}");
    }
    std::fs::read(file.path()).context("read edited temporary file")
}

fn fuzzy(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut score = 0;
    let mut chars = needle.chars();
    let mut target = chars.next()?;
    for (index, value) in haystack.to_lowercase().chars().enumerate() {
        if value == target {
            score += 100usize.saturating_sub(index);
            match chars.next() {
                Some(next) => target = next,
                None => return Some(score),
            }
        }
    }
    None
}

fn highlighted_markdown(source: &str) -> Text<'static> {
    let mut frontmatter = false;
    let mut fenced_code = false;
    let lines = source
        .split('\n')
        .map(|line| {
            if line == "---" {
                frontmatter = !frontmatter;
                return Line::from(Span::styled(
                    line.to_owned(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if frontmatter {
                if let Some((key, value)) = line.split_once(':') {
                    return Line::from(vec![
                        Span::styled(
                            format!("{key}:"),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(value.to_owned(), Style::default().fg(Color::Yellow)),
                    ]);
                }
                return Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            if line.trim_start().starts_with("```") {
                fenced_code = !fenced_code;
                return Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Blue),
                ));
            }
            let style = if fenced_code {
                Style::default().fg(Color::Blue)
            } else if line.starts_with('#') {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if line.starts_with('>') {
                Style::default().fg(Color::DarkGray)
            } else if line.contains("](") || line.contains("[[") {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            Line::from(Span::styled(line.to_owned(), style))
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

#[allow(clippy::too_many_lines)]
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let has_input = matches!(
        app.mode,
        Mode::Search | Mode::CreateTitle | Mode::Goto | Mode::ActionPicker
    );
    let input_height = if app.mode == Mode::Goto {
        u16::try_from((app.goto_choices().len() + 3).clamp(4, 10)).unwrap_or(10)
    } else {
        3
    };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_input {
            vec![
                Constraint::Length(4),
                Constraint::Length(input_height),
                Constraint::Min(1),
            ]
        } else {
            vec![Constraint::Length(4), Constraint::Min(1)]
        })
        .split(frame.area());
    let workspace = if app.workspace_id.is_empty() {
        "local".to_owned()
    } else {
        app.workspace_id.chars().take(12).collect()
    };
    let status = app.sync.as_ref().map_or_else(
        || "Offline · pending 0 · missing 0 · not converged".to_owned(),
        |sync| {
            format!(
                "{:?} · pending {} · missing {} · {}",
                sync.connectivity,
                sync.pending_operations,
                sync.missing_blobs.len(),
                if sync.converged {
                    "converged"
                } else {
                    "not converged"
                }
            )
        },
    );
    let header = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(2)])
        .split(vertical[0]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("xo · workspace {workspace} · {status}")),
            Line::from(format!(
                "conflicts {} · devices {} · diagnostics {}{}",
                app.conflicts.len(),
                app.devices.len(),
                app.diagnostics.len(),
                if app.message.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", app.message)
                }
            )),
        ])
        .style(Style::default().fg(Color::Cyan)),
        header[0],
    );
    let key_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(header[1]);
    for (area, lines) in key_columns.iter().zip([
        vec!["[↑↓/jk] select · [Tab] pane", "[Space] tag · [/] filter"],
        vec![
            "[Enter/e] edit · [c] create",
            "[d/u] del/restore · [g] goto",
        ],
        vec!["[a] actions · [T] tags · [J] pair", "[r] sync · [q] quit"],
    ]) {
        frame.render_widget(
            Paragraph::new(lines.join("\n")).style(Style::default().fg(Color::Cyan)),
            *area,
        );
    }
    if has_input {
        let (title, value) = match app.mode {
            Mode::Search => (
                "Filter notes · Enter apply · Esc close",
                format!("/{}", app.search),
            ),
            Mode::CreateTitle => (
                "New item title · Enter create and edit · Esc cancel",
                format!("Title: {}", app.create_title),
            ),
            Mode::Goto => {
                let choices = app.goto_choices();
                let menu = choices
                    .iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        format!(
                            "{} [{}] {}",
                            if index == app.goto_index { "→" } else { " " },
                            choice.prefix,
                            choice.label
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (
                    "Goto view · type shown prefix · ↑/↓ choose · Enter apply · Esc close",
                    format!("g{}\n{menu}", app.goto_input),
                )
            }
            Mode::ActionPicker => {
                let selected = app
                    .matching_actions()
                    .first()
                    .map_or("no matching action", |action| action.description.as_str());
                (
                    "Run action · Enter apply · Esc close",
                    format!(">{}  → {selected}", app.action_query),
                )
            }
            _ => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(value).block(Block::default().title(title).borders(Borders::ALL)),
            vertical[1],
        );
    }
    let content_area = vertical[usize::from(has_input) + 1];
    if app.mode == Mode::Pairing {
        render_pairing(frame, app, content_area);
        return;
    }
    let pane_constraints = if app.tags_visible {
        vec![
            Constraint::Percentage(22),
            Constraint::Percentage(35),
            Constraint::Percentage(43),
        ]
    } else {
        vec![Constraint::Percentage(45), Constraint::Percentage(55)]
    };
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(pane_constraints)
        .split(content_area);
    let notes_pane = usize::from(app.tags_visible);
    let preview_pane = notes_pane + 1;
    let selected = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    if app.tags_visible {
        let available_tags = app.available_tags();
        let tags = if available_tags.is_empty() {
            vec![ListItem::new("  No tags")]
        } else {
            available_tags
                .iter()
                .enumerate()
                .map(|(index, (tag, count))| {
                    let highlighted = app.pane == Pane::Tags && index == app.tag_index;
                    ListItem::new(format!(
                        "{} [{}] {} ({count})",
                        if highlighted { "▶" } else { " " },
                        if app.selected_tags.contains(tag) {
                            "x"
                        } else {
                            " "
                        },
                        tag,
                    ))
                    .style(
                        if highlighted || app.selected_tags.contains(tag) {
                            selected
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        frame.render_widget(
            List::new(tags).block(
                Block::default()
                    .title(format!("Tags · {} selected", app.selected_tags.len()))
                    .borders(Borders::ALL)
                    .border_style(if app.pane == Pane::Tags {
                        selected
                    } else {
                        Style::default()
                    }),
            ),
            panes[0],
        );
    }
    let selected_index = app.selected_index();
    let note_items = app
        .visible_notes()
        .iter()
        .enumerate()
        .map(|(index, note)| {
            let title = match note.frontmatter.get("title") {
                Some(FrontmatterValue::String(value)) => value.clone(),
                _ => note.id.to_string(),
            };
            ListItem::new(format!(
                "{} {title}",
                if Some(index) == selected_index {
                    "▶"
                } else {
                    " "
                }
            ))
            .style(if Some(index) == selected_index {
                selected
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(note_items).block(
            Block::default()
                .title(format!("Notes · {} visible", app.visible_notes().len()))
                .borders(Borders::ALL)
                .border_style(if app.pane == Pane::Notes {
                    selected
                } else {
                    Style::default()
                }),
        ),
        panes[notes_pane],
    );
    let (right_title, right_text) = match app.mode {
        Mode::Conflicts => (
            "Conflict history",
            Text::from(
                app.conflicts
                    .iter()
                    .map(|conflict| {
                        let revisions = app
                            .conflict_history
                            .get(&conflict.note_id)
                            .into_iter()
                            .flatten()
                            .map(|(id, revision)| {
                                format!(
                                    "  {} {}{}",
                                    id,
                                    revision.materialized_path,
                                    if revision.deleted { " [deleted]" } else { "" }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "{}\nwinner: {}\nconcurrent: {}\n{}",
                            conflict.note_id,
                            conflict.winning_revision,
                            conflict
                                .concurrent_revisions
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", "),
                            revisions
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
        ),
        Mode::Devices => (
            "Devices (V retires)",
            Text::from(
                app.devices
                    .iter()
                    .map(|device| {
                        format!(
                            "{}\n{}\ncapabilities: {}\nretired: {}",
                            device.label,
                            device.endpoint_id,
                            device
                                .capabilities
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", "),
                            device.retired_at.is_some()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
        ),
        Mode::Sync => (
            "Synchronization (R retries)",
            Text::from(format!(
                "operations\n{}\n\nmissing blobs\n{}\n\ndiagnostics\n{}",
                app.operations
                    .iter()
                    .map(|value| format!(
                        "{} {} {:?} attempts={} {}",
                        value.id,
                        value.kind,
                        value.status,
                        value.attempts,
                        value.last_error.as_deref().unwrap_or("")
                    ))
                    .collect::<Vec<_>>()
                    .join("\n"),
                app.sync
                    .as_ref()
                    .map(|value| value.missing_blobs.join("\n"))
                    .unwrap_or_default(),
                app.diagnostics
                    .iter()
                    .map(|value| format!("{} [{}] {}", value.path, value.code, value.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
        ),
        _ => (
            "Raw Markdown",
            if app.selected_note().is_none() {
                Text::from("No notes match the current view and filter.")
            } else {
                let raw = app.decrypted_preview.as_ref().map_or_else(
                    || app.preview(None).unwrap_or_else(|error| format!("{error}")),
                    |value| value.as_str().to_owned(),
                );
                highlighted_markdown(&raw)
            },
        ),
    };
    frame.render_widget(
        Paragraph::new(right_text).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(right_title)
                .borders(Borders::ALL)
                .border_style(if app.pane == Pane::Preview {
                    selected
                } else {
                    Style::default()
                }),
        ),
        panes[preview_pane],
    );
}

fn render_pairing(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let Some(pairing) = &app.pairing else {
        frame.render_widget(
            Paragraph::new("Pairing state is unavailable.").block(
                Block::default()
                    .title("Connect xo-syncd")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    };
    let error = if pairing.error.is_empty() {
        String::new()
    } else {
        format!("\n\nError: {}", pairing.error)
    };
    let text = match pairing.step {
        PairingStep::StateDirectory => format!(
            "Step 1 of 3 — Server location\n\n\
             Enter the state directory used by xo-syncd on the server.\n\n\
             > {}\n\n\
             Enter: generate invitation · Ctrl+U: clear · Esc: cancel{}",
            pairing.state_dir, error
        ),
        PairingStep::ServerCommand => {
            let command = if pairing.reveal_ticket {
                app.pairing_command().unwrap_or_default()
            } else {
                Zeroizing::new(server_pairing_commands(
                    &pairing.state_dir,
                    "<writable ticket hidden>",
                ))
            };
            format!(
                "Step 2 of 3 — Run on the server\n\n\
                 Copy and run these commands on the server. They stop xo-syncd, import this \
                 workspace as the xo service user, and restart the daemon.\n\n\
                 {}\n\n\
                 c: copy commands · F2: show/hide ticket · Enter: paste server output · Esc: cancel\
                 \n\nThe invitation is a writable capability. Keep it private.{error}",
                command.as_str()
            )
        }
        PairingStep::ServerOutput => {
            let output = if pairing.server_output.is_empty() {
                Zeroizing::new("<paste xo-admin output here>".to_owned())
            } else if pairing.reveal_ticket {
                pairing.server_output.clone()
            } else {
                Zeroizing::new("<server output hidden>".to_owned())
            };
            format!(
                "Step 3 of 3 — Complete pairing\n\n\
                 Paste the complete output from xo-admin import-ticket, or only its ticket= line.\n\n\
                 {}\n\n\
                 Enter: connect · F2: show/hide pasted output · Backspace: edit · Esc: cancel{error}",
                output.as_str()
            )
        }
        PairingStep::Connected => format!(
            "Server connected\n\n\
             Workspace: {}\n\n\
             The server ticket was accepted and synchronization has started. Future launches \
             resume from the stored peer relationship.\n\n\
             Enter or Esc: return to notes",
            app.workspace_id
        ),
    };
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            Block::default()
                .title("Connect xo-syncd")
                .borders(Borders::ALL),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use xo_core::behavior::{
        ActionDescriptor, ActionEffect, Capability, Predicate, SubviewDescriptor, ViewDescriptor,
    };

    fn fixture() -> App {
        let mut behavior = WorkspaceBehavior {
            views: vec![ViewDescriptor {
                id: "notes".into(),
                name: "Notes".into(),
                key: Some("n".into()),
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: None,
                sort_field: Some("title".into()),
                descending: false,
                preview: None,
                predicate: Predicate::Always,
                subviews: vec![],
            }],
            actions: vec![ActionDescriptor {
                id: "done".into(),
                description: "Mark done".into(),
                predicate: Predicate::Always,
                effects: vec![ActionEffect::AddTag { tag: "done".into() }],
            }],
            ..WorkspaceBehavior::default()
        };
        behavior
            .capability_grants
            .insert("done".into(), BTreeSet::from([Capability::MutateNote]));
        App::new(
            behavior,
            vec![Note {
                id: NoteId::new("note001"),
                frontmatter: Frontmatter::from([(
                    "title".into(),
                    FrontmatterValue::String("First".into()),
                )]),
                body: "Hello **world**".into(),
                path: "first.md".into(),
            }],
        )
    }

    #[test]
    fn shell_renders_navigation_preview_search_and_action_picker() {
        let mut app = fixture();
        app.search = "fir".into();
        app.action_query = "dn".into();
        assert_eq!(app.matching_actions()[0].id, "done");
        app.run_action("done").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(screen.contains("Tags"));
        assert!(screen.contains("First"));
        assert!(screen.contains("Hello **world**"));
        assert!(screen.contains("Offline"));
        assert!(screen.contains("[Enter/e] edit"));
        assert!(!screen.contains("pending=0"));
    }

    #[test]
    fn server_pairing_builds_safe_commands_and_parses_admin_output() {
        let commands =
            server_pairing_commands("/var/lib/xo syncd", "ticket'with-sensitive-content");
        assert_eq!(
            commands,
            "sudo systemctl stop xo-syncd\n\
             sudo -u xo xo-admin import-ticket '/var/lib/xo syncd' \
             'ticket'\"'\"'with-sensitive-content'\n\
             sudo systemctl start xo-syncd"
        );
        assert_eq!(
            ticket_from_server_output(
                "workspace_id=workspace123\n\
                 ticket=server-ticket-123\n"
            )
            .as_ref()
            .map(|value| value.as_str()),
            Some("server-ticket-123")
        );
        assert_eq!(
            ticket_from_server_output("server-ticket-456")
                .as_ref()
                .map(|value| value.as_str()),
            Some("server-ticket-456")
        );
        assert!(ticket_from_server_output("workspace_id=workspace123").is_none());
    }

    #[test]
    fn server_pairing_renders_each_step_without_revealing_tickets_by_default() {
        let mut app = fixture();
        app.workspace_id = "workspace123".into();
        app.start_server_pairing();
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let state_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(state_screen.contains("Step 1 of 3"));
        assert!(state_screen.contains("/var/lib/xo-syncd"));

        app.set_pairing_invitation("client-secret-ticket".into());
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let command_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(command_screen.contains("Step 2 of 3"));
        assert!(command_screen.contains("xo-admin import-ticket"));
        assert!(command_screen.contains("<writable ticket hidden>"));
        assert!(!command_screen.contains("client-secret-ticket"));

        let pairing = app.pairing.as_mut().unwrap();
        pairing.step = PairingStep::ServerOutput;
        pairing.server_output = Zeroizing::new("ticket=server-secret-ticket".into());
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let output_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(output_screen.contains("Step 3 of 3"));
        assert!(output_screen.contains("<server output hidden>"));
        assert!(!output_screen.contains("server-secret-ticket"));

        app.pairing.as_mut().unwrap().step = PairingStep::Connected;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let connected_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(connected_screen.contains("Server connected"));
        assert!(connected_screen.contains("workspace123"));
    }

    #[test]
    fn filter_keeps_selection_valid_and_goto_selects_a_view_by_unique_prefix() {
        let mut app = fixture();
        app.selected = 42;
        assert_eq!(app.selected_note().unwrap().id.as_str(), "note001");
        app.search = "missing".into();
        assert!(app.selected_note().is_none());
        app.search.clear();
        app.behavior.views.extend([
            ViewDescriptor {
                id: "news".into(),
                name: "News".into(),
                key: None,
                show_tags: false,
                title_field: "title".into(),
                subtitle_field: None,
                sort_field: None,
                descending: false,
                preview: None,
                predicate: Predicate::Always,
                subviews: vec![],
            },
            ViewDescriptor {
                id: "books".into(),
                name: "Books".into(),
                key: None,
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: None,
                sort_field: None,
                descending: false,
                preview: None,
                predicate: Predicate::Always,
                subviews: vec![SubviewDescriptor {
                    id: "reading".into(),
                    name: "Reading".into(),
                    predicate: Predicate::Always,
                }],
            },
        ]);
        let choices = app.goto_choices();
        assert_eq!(
            choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.prefix.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Notes", "no"),
                ("News", "ne"),
                ("Books", "b"),
                ("Books / Reading", "r"),
            ]
        );
        app.goto_input = "ne".into();
        assert!(app.goto_is_unambiguous());
        assert!(app.choose_goto());
        assert_eq!(app.active_view, "news");
        assert!(!app.tags_visible);
    }

    #[test]
    fn required_creation_fields_and_default_preview_are_present() {
        let frontmatter = required_frontmatter(
            Frontmatter::from([("title".into(), FrontmatterValue::String("Kept".into()))]),
            "note001",
            "2026-07-22T10:00:00Z",
        );
        for field in ["id", "created", "tags", "title", "type"] {
            assert!(frontmatter.contains_key(field));
        }
        let mut app = fixture();
        app.notes[0].frontmatter = frontmatter;
        let preview = app.preview(None).unwrap();
        assert!(preview.starts_with("---\n"));
        assert!(preview.contains("title: Kept"));
        assert!(preview.contains("type: note"));
        assert!(preview.contains("created: 2026-07-22T10:00:00Z"));
        assert!(preview.contains("Hello **world**"));
    }

    #[test]
    fn search_and_goto_menu_render_between_header_and_content() {
        let mut app = fixture();
        app.mode = Mode::Search;
        app.search = "first".into();
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let search_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(search_screen.contains("Filter notes"));
        assert!(search_screen.contains("/first"));

        app.mode = Mode::Goto;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let view_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(view_screen.contains("Goto view"));
        assert!(view_screen.contains("→ [n] Notes"));

        app.mode = Mode::CreateTitle;
        app.create_title = "A new thought".into();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let create_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(create_screen.contains("New item title"));
        assert!(create_screen.contains("Title: A new thought"));
    }

    #[test]
    fn raw_markdown_highlighting_preserves_text_and_marks_syntax() {
        let raw = "---\ntitle: Example\n---\n# Heading\n[link](target)\n";
        let highlighted = highlighted_markdown(raw);
        assert_eq!(highlighted.lines[0].spans[0].style.fg, Some(Color::Magenta));
        assert_eq!(highlighted.lines[1].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(highlighted.lines[3].spans[0].style.fg, Some(Color::Green));
        assert_eq!(highlighted.lines[4].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(
            highlighted
                .lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            raw
        );
    }

    #[test]
    fn delete_and_restore_retain_note() {
        let mut app = fixture();
        let note = app.delete_selected().unwrap();
        assert!(app.notes.is_empty());
        app.restore(&note.id).unwrap();
        assert_eq!(app.notes.len(), 1);
    }

    #[test]
    fn encrypted_preview_and_temp_edit_require_the_passphrase() {
        let mut app = fixture();
        app.notes[0].body = encryption::encrypt("note001", "correct", "secret").unwrap();
        assert!(app.preview(None).is_err());
        assert!(app.unlock_preview("wrong").is_err());
        app.unlock_preview("correct").unwrap();
        assert!(
            app.decrypted_preview
                .as_deref()
                .is_some_and(|preview| preview.contains("secret"))
        );
        let args = [
            OsStr::new("-c"),
            OsStr::new("printf changed > \"$1\""),
            OsStr::new("_"),
        ];
        let note = app
            .edit_encrypted_with("correct", OsStr::new("sh"), &args)
            .unwrap();
        assert_eq!(
            encryption::decrypt("note001", "correct", &note.body).unwrap(),
            "changed"
        );
    }

    #[test]
    fn external_editor_may_atomically_replace_the_temporary_file() {
        let args = [
            OsStr::new("-c"),
            OsStr::new("printf changed > \"$1.next\"; mv \"$1.next\" \"$1\""),
            OsStr::new("_"),
        ];
        let edited = external_edit_with(OsStr::new("sh"), &args, b"initial").unwrap();
        assert_eq!(edited, b"changed");
    }

    #[test]
    fn multi_tag_filter_is_conjunctive() {
        let mut app = fixture();
        app.notes.push(Note {
            id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([
                ("title".into(), FrontmatterValue::String("Second".into())),
                (
                    "tags".into(),
                    FrontmatterValue::Sequence(vec![
                        FrontmatterValue::String("a".into()),
                        FrontmatterValue::String("b".into()),
                    ]),
                ),
            ]),
            body: String::new(),
            path: "second.md".into(),
        });
        app.toggle_tag("a");
        app.toggle_tag("b");
        assert_eq!(app.visible_notes().len(), 1);
        assert_eq!(app.visible_notes()[0].id.as_str(), "note002");
    }

    #[test]
    fn tag_pane_lists_counts_and_toggles_the_highlighted_filter() {
        let mut app = fixture();
        app.notes[0].frontmatter.insert(
            "tags".into(),
            FrontmatterValue::Sequence(vec![FrontmatterValue::String("rust".into())]),
        );
        assert_eq!(app.available_tags(), vec![("rust".into(), 1)]);
        app.pane = Pane::Tags;
        app.toggle_highlighted_tag();
        assert!(app.selected_tags.contains("rust"));

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(screen.contains("[x] rust (1)"));

        app.toggle_tags_visible();
        assert_eq!(app.pane, Pane::Notes);
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(!screen.contains("Tags · 1 selected"));
    }

    #[test]
    fn tag_counts_are_faceted_by_view_search_and_selected_tags() {
        let mut app = fixture();
        app.behavior.views[0].predicate = Predicate::FieldEquals {
            field: "type".into(),
            value: "note".into(),
        };
        app.set_view("notes");
        app.notes = vec![
            tagged_note("note001", "Alpha", "note", &["rust", "work"]),
            tagged_note("note002", "Beta", "note", &["rust", "personal"]),
            tagged_note(
                "note003",
                "Alpha document",
                "document",
                &["rust", "personal"],
            ),
            tagged_note("note004", "Gamma", "note", &["work"]),
        ];

        assert_eq!(
            app.available_tags(),
            vec![
                ("personal".into(), 1),
                ("rust".into(), 2),
                ("work".into(), 2),
            ]
        );
        app.toggle_tag("rust");
        assert_eq!(
            app.available_tags(),
            vec![
                ("personal".into(), 1),
                ("rust".into(), 2),
                ("work".into(), 1),
            ]
        );
        app.search = "Alpha".into();
        assert_eq!(
            app.available_tags(),
            vec![("rust".into(), 1), ("work".into(), 1)]
        );
    }

    fn tagged_note(id: &str, title: &str, item_type: &str, tags: &[&str]) -> Note {
        Note {
            id: NoteId::new(id),
            frontmatter: Frontmatter::from([
                ("title".into(), FrontmatterValue::String(title.into())),
                ("type".into(), FrontmatterValue::String(item_type.into())),
                (
                    "tags".into(),
                    FrontmatterValue::Sequence(
                        tags.iter()
                            .map(|tag| FrontmatterValue::String((*tag).into()))
                            .collect(),
                    ),
                ),
            ]),
            body: String::new(),
            path: format!("{id}.md"),
        }
    }
}
