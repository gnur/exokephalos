use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::process::Command;

use anyhow::{Context, Result, bail};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tempfile::NamedTempFile;
use xo_core::behavior::{
    Query, TemplateInputs, WorkspaceBehavior, render_preview, render_template,
};
use xo_core::domain::{DeviceRecord, Frontmatter, FrontmatterValue};
use xo_core::encryption;
use xo_core::projection::Diagnostic;
use xo_core::sync_state::{DurableOperation, SyncStatus};
use xo_core::{Conflict, Note, NoteId, NoteRevision, RevisionId};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pane {
    Views,
    Notes,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Normal,
    Search,
    ActionPicker,
    Conflicts,
    Devices,
    Sync,
}

pub struct App {
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
    pub pane: Pane,
    pub mode: Mode,
    pub selected: usize,
    pub action_query: String,
    pub message: String,
    pub decrypted_preview: Option<Zeroizing<String>>,
    sort_descending: bool,
}

impl App {
    pub fn new(behavior: WorkspaceBehavior, notes: Vec<Note>) -> Self {
        let active_view = behavior.default_view.clone();
        Self {
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
            pane: Pane::Notes,
            mode: Mode::Normal,
            selected: 0,
            action_query: String::new(),
            message: String::new(),
            decrypted_preview: None,
            sort_descending: false,
        }
    }

    pub fn visible_notes(&self) -> Vec<&Note> {
        let mut notes = self
            .behavior
            .query(
                &self.notes,
                &Query {
                    view: self.active_view.clone(),
                    subview: self.active_subview.clone(),
                    title: (!self.search.is_empty()).then(|| self.search.clone()),
                    tags: self.selected_tags.clone(),
                    limit: None,
                },
            )
            .unwrap_or_default();
        if self.sort_descending {
            notes.reverse();
        }
        notes
    }

    pub fn selected_note(&self) -> Option<&Note> {
        self.visible_notes().get(self.selected).copied()
    }
    pub fn next_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Views => Pane::Notes,
            Pane::Notes => Pane::Preview,
            Pane::Preview => Pane::Views,
        };
    }
    pub fn previous_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Views => Pane::Preview,
            Pane::Notes => Pane::Views,
            Pane::Preview => Pane::Notes,
        };
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
    pub fn set_view(&mut self, id: &str) {
        id.clone_into(&mut self.active_view);
        self.active_subview = None;
        self.selected = 0;
    }
    pub fn set_subview(&mut self, id: Option<String>) {
        self.active_subview = id;
        self.selected = 0;
    }
    pub fn toggle_tag(&mut self, tag: &str) {
        if !self.selected_tags.remove(tag) {
            self.selected_tags.insert(tag.to_owned());
        }
        self.selected = 0;
    }
    pub fn toggle_sort(&mut self) {
        self.sort_descending = !self.sort_descending;
    }

    pub fn create_from_template(
        &mut self,
        template_id: &str,
        inputs: &TemplateInputs,
        path: String,
    ) -> Result<Note> {
        let template = self
            .behavior
            .templates
            .iter()
            .find(|value| value.id == template_id)
            .context("unknown template")?;
        let rendered = render_template(&template.content, inputs);
        let parsed = xo_core::markdown::parse(&rendered)?;
        let note = Note {
            id: NoteId::new(inputs.id.clone()),
            frontmatter: parsed.frontmatter.unwrap_or_default(),
            body: parsed.body,
            path,
        };
        self.notes.push(note.clone());
        self.selected = self.notes.len().saturating_sub(1);
        Ok(note)
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
        let template = self
            .behavior
            .views
            .iter()
            .find(|view| view.id == self.active_view)
            .and_then(|view| view.preview.as_deref())
            .unwrap_or("{{Body}}");
        Ok(render_preview(template, &visible))
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
    let mut output = Vec::new();
    file.reopen()?.read_to_end(&mut output)?;
    Ok(output)
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

#[allow(clippy::too_many_lines)]
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(frame.area());
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(22),
            Constraint::Percentage(35),
            Constraint::Percentage(43),
        ])
        .split(vertical[0]);
    let selected = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let mut views = vec![ListItem::new(Line::from("0  All").style(
        if app.active_view == "all" {
            selected
        } else {
            Style::default()
        },
    ))];
    views.extend(app.behavior.views.iter().map(|view| {
        ListItem::new(format!(
            "{}  {}",
            view.key.as_deref().unwrap_or("·"),
            view.name
        ))
        .style(if view.id == app.active_view {
            selected
        } else {
            Style::default()
        })
    }));
    let tags = if app.selected_tags.is_empty() {
        String::new()
    } else {
        format!(
            "\nTags: {}",
            app.selected_tags
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    frame.render_widget(
        List::new(views).block(
            Block::default()
                .title(format!("Views{tags}"))
                .borders(Borders::ALL)
                .border_style(if app.pane == Pane::Views {
                    selected
                } else {
                    Style::default()
                }),
        ),
        panes[0],
    );
    let note_items = app
        .visible_notes()
        .iter()
        .enumerate()
        .map(|(index, note)| {
            let title = match note.frontmatter.get("title") {
                Some(FrontmatterValue::String(value)) => value.clone(),
                _ => note.id.to_string(),
            };
            ListItem::new(title).style(if index == app.selected {
                selected
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(note_items).block(
            Block::default()
                .title(format!(
                    "Notes [{}] /{}",
                    app.search,
                    app.visible_notes().len()
                ))
                .borders(Borders::ALL)
                .border_style(if app.pane == Pane::Notes {
                    selected
                } else {
                    Style::default()
                }),
        ),
        panes[1],
    );
    let (right_title, right_text) = match app.mode {
        Mode::Conflicts => (
            "Conflict history",
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
        Mode::Devices => (
            "Devices (V retires)",
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
        Mode::Sync => (
            "Synchronization (R retries)",
            format!(
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
            ),
        ),
        _ => (
            "Markdown preview",
            app.decrypted_preview.as_ref().map_or_else(
                || app.preview(None).unwrap_or_else(|error| format!("{error}")),
                |value| value.as_str().to_owned(),
            ),
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
        panes[2],
    );
    let status = app.sync.as_ref().map_or_else(
        || "offline".to_owned(),
        |sync| {
            format!(
                "{:?} pending={} missing={} converged={}",
                sync.connectivity,
                sync.pending_operations,
                sync.missing_blobs.len(),
                sync.converged
            )
        },
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{status} | conflicts={} devices={} diagnostics={} | {}",
            app.conflicts.len(),
            app.devices.len(),
            app.diagnostics.len(),
            app.message
        )),
        vertical[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use xo_core::behavior::{
        ActionDescriptor, ActionEffect, Capability, Predicate, ViewDescriptor,
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
        assert!(screen.contains("Views"));
        assert!(screen.contains("First"));
        assert!(screen.contains("Hello **world**"));
        assert!(screen.contains("offline"));
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
        assert_eq!(
            app.decrypted_preview.as_deref().map(String::as_str),
            Some("secret")
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
}
