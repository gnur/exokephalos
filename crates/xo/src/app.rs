use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write;
use std::process::Command;

use anyhow::{Context, Result, bail};
use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tempfile::Builder as TempFileBuilder;
use xo::steel_plugin::PluginChoice;
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
    Leader,
    Search,
    CreateTitle,
    CreateEncryptedTitle,
    Goto,
    ViewPicker,
    ActionPicker,
    CaptureUrl,
    PluginInput,
    PluginResults,
    Conflicts,
    Devices,
    Sync,
    Pairing,
    MobilePairing,
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

pub struct MobilePairing {
    pub setup_url: Zeroizing<String>,
    pub qr: Zeroizing<String>,
    pub host: String,
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
    pub capture_url: String,
    pub plugin_input: String,
    pub plugin_action: Option<String>,
    pub plugin_prompt: String,
    pub plugin_results: Vec<PluginChoice>,
    pub plugin_index: usize,
    pub create_title: String,
    pub goto_input: String,
    pub goto_index: usize,
    pub message: String,
    pub pwa_url: String,
    pub leader_key: char,
    pub pairing: Option<ServerPairing>,
    pub mobile_pairing: Option<MobilePairing>,
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
            capture_url: String::new(),
            plugin_input: String::new(),
            plugin_action: None,
            plugin_prompt: String::new(),
            plugin_results: vec![],
            plugin_index: 0,
            create_title: String::new(),
            goto_input: String::new(),
            goto_index: 0,
            message: String::new(),
            pwa_url: "https://xo.exokephalos.dev/".to_owned(),
            leader_key: ' ',
            pairing: None,
            mobile_pairing: None,
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
    pub fn focus_right(&mut self) {
        self.pane = match self.pane {
            Pane::Tags => Pane::Notes,
            Pane::Notes | Pane::Preview => Pane::Preview,
        };
    }
    pub fn focus_left(&mut self) {
        self.pane = match self.pane {
            Pane::Tags => Pane::Tags,
            Pane::Notes if self.tags_visible => Pane::Tags,
            Pane::Notes | Pane::Preview => Pane::Notes,
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

    pub fn cycle_subview(&mut self, forward: bool) -> bool {
        let Some(view) = self
            .behavior
            .views
            .iter()
            .find(|view| view.id == self.active_view)
        else {
            return false;
        };
        if view.subviews.is_empty() {
            return false;
        }
        let current = self
            .active_subview
            .as_ref()
            .and_then(|id| view.subviews.iter().position(|subview| &subview.id == id));
        let next = if forward {
            current.map_or(0, |index| (index + 1) % view.subviews.len())
        } else {
            current.map_or(view.subviews.len() - 1, |index| {
                index.checked_sub(1).unwrap_or(view.subviews.len() - 1)
            })
        };
        self.set_subview(Some(view.subviews[next].id.clone()));
        true
    }

    pub fn subview_header(&self) -> String {
        let Some(view) = self
            .behavior
            .views
            .iter()
            .find(|view| view.id == self.active_view)
        else {
            return String::new();
        };
        if view.subviews.is_empty() {
            return String::new();
        }
        view.subviews
            .iter()
            .map(|subview| {
                if self.active_subview.as_deref() == Some(subview.id.as_str()) {
                    format!("[{}]", subview.name)
                } else {
                    subview.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    pub fn main_view_choices(&self) -> Vec<ViewChoice> {
        let mut used = BTreeSet::new();
        self.behavior
            .views
            .iter()
            .enumerate()
            .map(|(index, view)| {
                let candidates = view
                    .key
                    .iter()
                    .flat_map(|key| key.chars())
                    .chain(view.name.chars())
                    .chain(view.id.chars())
                    .chain("abcdefghijklmnopqrstuvwxyz0123456789".chars())
                    .chain((33_u8..=126).map(char::from));
                let key = candidates
                    .map(|candidate| candidate.to_ascii_lowercase())
                    .find(|candidate| !candidate.is_whitespace() && used.insert(*candidate))
                    .unwrap_or_else(|| {
                        let offset = u32::try_from(index).unwrap_or(0);
                        char::from_u32(0xe000_u32.saturating_add(offset)).unwrap_or('?')
                    });
                ViewChoice {
                    label: view.name.clone(),
                    view: view.id.clone(),
                    subview: None,
                    navigation_key: key.to_string(),
                    prefix: key.to_string(),
                }
            })
            .collect()
    }

    pub fn choose_main_view(&mut self) -> bool {
        if let Some(choice) = self.main_view_choices().get(self.goto_index).cloned() {
            self.set_view(&choice.view);
            return true;
        }
        false
    }

    pub fn choose_main_view_key(&mut self, key: char) -> bool {
        let key = key.to_ascii_lowercase();
        if let Some(choice) = self
            .main_view_choices()
            .into_iter()
            .find(|choice| choice.navigation_key.starts_with(key))
        {
            self.set_view(&choice.view);
            return true;
        }
        false
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

    pub fn replace_selected(&mut self, frontmatter: Frontmatter, body: String) -> Option<Note> {
        let id = self.selected_note()?.id.clone();
        let note = self.notes.iter_mut().find(|note| note.id == id)?;
        note.frontmatter = frontmatter;
        note.body = body;
        note.path = xo_core::projection::canonical_note_path(&note.id, &note.frontmatter);
        let changed = note.clone();
        self.decrypted_preview = None;
        Some(changed)
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
        let note = self.selected_note();
        let needle = self.action_query.to_lowercase();
        let mut actions = self
            .behavior
            .actions
            .iter()
            .filter(|action| {
                note.is_some_and(|note| action.predicate.matches(note))
                    || (note.is_none()
                        && action.plugin.is_some()
                        && action.predicate == xo_core::behavior::Predicate::Always)
            })
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
        if self
            .behavior
            .actions
            .iter()
            .find(|action| action.id == id)
            .is_some_and(|action| action.plugin.is_some())
        {
            bail!("action {id} requires its native host plugin");
        }
        let note_id = self.selected_note().context("no selected note")?.id.clone();
        let note = self
            .notes
            .iter_mut()
            .find(|note| note.id == note_id)
            .context("selected note disappeared")?;
        let now = xo_core::timestamp::format(xo_core::timestamp::now_local()?)?;
        self.behavior.apply_action(note, id, &now)?;
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
        let document = Zeroizing::new(xo_core::markdown::render(&note.frontmatter, &plaintext)?);
        let edited = Zeroizing::new(external_edit_with(program, args, document.as_bytes())?);
        let parsed = xo_core::markdown::parse(std::str::from_utf8(&edited)?)?;
        let created = match note.frontmatter.get("created") {
            Some(FrontmatterValue::String(value)) => value.clone(),
            _ => anyhow::bail!("encrypted note has no creation timestamp"),
        };
        let frontmatter = required_frontmatter(
            parsed.frontmatter.unwrap_or_default(),
            note.id.as_str(),
            &created,
        );
        let plaintext = Zeroizing::new(parsed.body);
        let encrypted = encryption::encrypt(note.id.as_str(), passphrase, &plaintext)?;
        self.replace_selected(frontmatter, encrypted)
            .context("selected note disappeared")
    }

    pub fn start_mobile_pairing(&mut self, ticket: &str) -> Result<()> {
        let setup_url = mobile_setup_url(&self.pwa_url, ticket)?;
        let code = QrCode::with_error_correction_level(setup_url.as_bytes(), EcLevel::L)
            .context("encode mobile setup QR code")?;
        let qr = code.render::<Dense1x2>().build();
        let host = url::Url::parse(&self.pwa_url)?
            .host_str()
            .context("pwa-url has no host")?
            .to_owned();
        self.mobile_pairing = Some(MobilePairing {
            setup_url,
            qr: Zeroizing::new(qr),
            host,
        });
        self.mode = Mode::MobilePairing;
        Ok(())
    }

    pub fn cancel_mobile_pairing(&mut self) {
        self.mobile_pairing = None;
        self.mode = Mode::Normal;
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

    pub fn user_syncd_command(&self) -> Option<Zeroizing<String>> {
        let ticket = self.pairing.as_ref()?.invitation.as_deref()?;
        Some(Zeroizing::new(user_syncd_install_command(ticket)))
    }

    pub fn pairing_invitation(&self) -> Option<Zeroizing<String>> {
        Some(Zeroizing::new(
            self.pairing.as_ref()?.invitation.as_deref()?.to_owned(),
        ))
    }

    pub fn pairing_ticket(&self) -> Option<Zeroizing<String>> {
        let pairing = self.pairing.as_ref()?;
        ticket_from_server_output(&pairing.server_output)
    }
}

pub fn mobile_setup_url(base: &str, ticket: &str) -> Result<Zeroizing<String>> {
    let mut url = url::Url::parse(base).context("parse pwa-url")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("pwa-url must be an HTTPS origin");
    }
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("ticket", ticket)
        .finish();
    url.set_fragment(Some(&fragment));
    Ok(Zeroizing::new(url.to_string()))
}

pub fn server_pairing_commands(state_dir: &str, ticket: &str) -> String {
    format!(
        "sudo systemctl stop xo-syncd\nsudo -u xo xo-admin import-ticket {} {}\nsudo systemctl start xo-syncd",
        shell_quote(state_dir),
        shell_quote(ticket)
    )
}

pub fn user_syncd_install_command(ticket: &str) -> String {
    format!(
        "curl -fsSL https://xo.exokephalos.dev/install.sh | XO_SYNC_TICKET={} bash",
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

pub fn required_frontmatter(frontmatter: Frontmatter, id: &str, created: &str) -> Frontmatter {
    xo_core::markdown::required_frontmatter(frontmatter, id, created)
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
    let mut file = TempFileBuilder::new()
        .suffix(".xo.md")
        .tempfile()
        .context("create secure editor file")?;
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
        Mode::Search
            | Mode::CreateTitle
            | Mode::CreateEncryptedTitle
            | Mode::Goto
            | Mode::ViewPicker
            | Mode::ActionPicker
            | Mode::CaptureUrl
            | Mode::PluginInput
            | Mode::PluginResults
    );
    let input_height = match app.mode {
        Mode::Goto => u16::try_from((app.goto_choices().len() + 3).clamp(4, 10)).unwrap_or(10),
        Mode::ViewPicker => {
            u16::try_from((app.main_view_choices().len() + 2).clamp(3, 10)).unwrap_or(10)
        }
        Mode::PluginResults => {
            u16::try_from((app.plugin_results.len() + 2).clamp(3, 9)).unwrap_or(9)
        }
        _ => 3,
    };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_input {
            vec![
                Constraint::Length(1),
                Constraint::Length(input_height),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
        } else {
            vec![
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
        })
        .split(frame.area());
    let view_name = app
        .behavior
        .views
        .iter()
        .find(|view| view.id == app.active_view)
        .map_or(app.active_view.as_str(), |view| view.name.as_str());
    let subviews = app.subview_header();
    let header = if subviews.is_empty() {
        format!("xo {} · {}", xo_core::version::VERSION, view_name)
    } else {
        format!(
            "xo {} · {} · {}",
            xo_core::version::VERSION,
            view_name,
            subviews
        )
    };
    frame.render_widget(
        Paragraph::new(header).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        vertical[0],
    );
    let leader = if app.leader_key == ' ' {
        "Space".to_owned()
    } else {
        app.leader_key.to_string()
    };
    let mut footer = format!(
        "[{leader}] menu · [g] views · [/] search · [e/Enter] edit · [c/C] create/encrypted · [d] delete · [u] restore · [q] quit"
    );
    if !app.message.is_empty() {
        footer.push_str(" · ");
        footer.push_str(&app.message);
    }
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        vertical[vertical.len() - 1],
    );
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
            Mode::CreateEncryptedTitle => (
                "New encrypted item title · Enter create and edit · Esc cancel",
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
                    "Goto view · type shown prefix · Enter apply · Esc close",
                    format!("g{}\n{menu}", app.goto_input),
                )
            }
            Mode::ViewPicker => {
                let menu = app
                    .main_view_choices()
                    .iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        format!(
                            "{} [{}] {}",
                            if index == app.goto_index { "→" } else { " " },
                            choice.navigation_key,
                            choice.label
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ("Switch view · key or ↑/↓ and Enter · Esc close", menu)
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
            Mode::CaptureUrl => (
                "Capture URL · Enter fetch · Esc cancel",
                format!("URL: {}", app.capture_url),
            ),
            Mode::PluginInput => (
                "Steel plugin · Enter run · Esc cancel",
                format!("{}: {}", app.plugin_prompt, app.plugin_input),
            ),
            Mode::PluginResults => {
                let choices = app
                    .plugin_results
                    .iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        format!(
                            "{} [{}] {}",
                            if index == app.plugin_index {
                                "→"
                            } else {
                                " "
                            },
                            index + 1,
                            choice.label
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (
                    "Plugin results · 1-9 select · Enter add · Esc cancel",
                    choices,
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
    if app.mode == Mode::MobilePairing {
        render_mobile_pairing(frame, app, content_area);
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
            "Preview",
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
    if app.mode == Mode::Leader {
        render_leader_menu(frame);
    }
}

fn render_leader_menu(frame: &mut Frame<'_>) {
    let entries = [
        "a  actions",
        "v  choose view",
        "c  config",
        "x  conflicts",
        "i  devices",
        "m  setup mobile client",
        "r  refresh sync",
        "o  reverse sort",
        "j  server setup/status",
        "s  synchronization status",
        "t  toggle tags",
        "p  unlock preview",
    ];
    let height = u16::try_from(entries.len()).unwrap_or(u16::MAX);
    let area = centered_rect(34, height.saturating_add(2), frame.area());
    let menu = Text::from(entries.into_iter().map(Line::raw).collect::<Vec<_>>());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(menu).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        area,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn render_mobile_pairing(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let Some(pairing) = &app.mobile_pairing else {
        return;
    };
    let qr_width = pairing
        .qr
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default();
    let qr_height = pairing.qr.lines().count();
    let fits = usize::from(area.width.saturating_sub(2)) >= qr_width
        && usize::from(area.height.saturating_sub(6)) >= qr_height;
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "Scan with your phone to open {} and join this workspace",
                pairing.host
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw("The QR contains a writable capability. Keep it private."),
        Line::raw(""),
    ];
    if fits {
        lines.extend(pairing.qr.lines().map(|line| Line::raw(line.to_owned())));
    } else {
        lines.push(Line::raw(format!(
            "Enlarge the terminal to at least {} columns × {} rows to display the QR code.",
            qr_width.saturating_add(2),
            qr_height.saturating_add(10)
        )));
    }
    lines.extend([
        Line::raw(""),
        Line::raw("c: copy setup link · Esc/Enter: close"),
    ]);
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(
            Block::default()
                .title("Mobile PWA setup")
                .borders(Borders::ALL),
        ),
        area,
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
            let invitation = if pairing.reveal_ticket {
                app.pairing_invitation().unwrap_or_default()
            } else {
                Zeroizing::new("<writable ticket hidden>".to_owned())
            };
            let installer_command = if pairing.reveal_ticket {
                app.user_syncd_command().map_or_else(
                    || "<installer command unavailable>".to_owned(),
                    |value| value.to_string(),
                )
            } else {
                "<show ticket with F2 to reveal installer command>".to_owned()
            };
            format!(
                "Step 2 of 3 — Add this workspace to the server\n\n\
                 Open http://127.0.0.1:9464/setup on the server. If xo-syncd is remote, \
                 forward that address over SSH first.\n\n\
                 Workspace ID: {}\n\
                 Operator token: {}/operator.token\n\
                 Writable ticket: {}\n\n\
                 User-unit installer: {}\n\n\
                 Enter those values in the setup page. It returns a server ticket.\n\n\
                 c: copy ticket · C: copy CLI fallback · U: copy user-unit installer · \
                 F2: show/hide ticket · Enter: paste server ticket · Esc: cancel\n\n\
                 The invitation is a writable capability. Keep it private.{error}",
                app.workspace_id,
                pairing.state_dir,
                invitation.as_str(),
                installer_command
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
                 Paste the server ticket returned by the setup page. The complete page output \
                 or a ticket= line is also accepted.\n\n\
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
                effects: vec![
                    ActionEffect::AddTag { tag: "done".into() },
                    ActionEffect::SetFieldNow {
                        field: "finished".into(),
                    },
                ],
                plugin: None,
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
        let changed = app.run_action("done").unwrap();
        let finished = match changed.frontmatter.get("finished") {
            Some(FrontmatterValue::String(value)) => value,
            value => panic!("expected finished timestamp, got {value:?}"),
        };
        time::OffsetDateTime::parse(finished, &time::format_description::well_known::Rfc3339)
            .unwrap();
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
        assert!(screen.contains(&format!("xo {}", xo_core::version::VERSION)));
        assert!(screen.contains("Preview"));
        assert!(screen.contains("[Space] menu"));
        assert!(screen.contains("[/] search"));
        assert!(screen.contains("[e/Enter] edit"));
        assert!(!screen.contains("Offline"));
        assert!(!screen.contains("↑↓/jk"));
    }

    #[test]
    fn leader_popup_lists_commands_and_uses_the_configured_key_in_the_footer() {
        let mut app = fixture();
        app.leader_key = ',';
        app.mode = Mode::Leader;
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(screen.contains("[,] menu"));
        assert!(!screen.contains("Leader menu"));
        assert!(screen.contains("t  toggle tags"));
        assert!(screen.contains("v  choose view"));
        assert!(screen.contains("a  actions"));
        assert!(screen.contains("c  config"));
        assert!(screen.contains("m  setup mobile client"));
        assert!(screen.contains("j  server setup/status"));
        assert!(screen.contains("s  synchronization"));
    }

    #[test]
    fn mobile_setup_uses_a_fragment_and_supports_a_custom_host() {
        let link = mobile_setup_url(
            "https://notes.example.test/",
            "writable ticket/with secrets",
        )
        .unwrap();
        let parsed = url::Url::parse(&link).unwrap();
        assert_eq!(parsed.host_str(), Some("notes.example.test"));
        assert_eq!(parsed.path(), "/");
        assert!(parsed.query().is_none());
        let parameters = url::form_urlencoded::parse(parsed.fragment().unwrap().as_bytes())
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            parameters.get("ticket").unwrap().as_ref(),
            "writable ticket/with secrets"
        );

        let mut app = fixture();
        app.pwa_url = "https://notes.example.test/".into();
        app.start_mobile_pairing("writable ticket/with secrets")
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(160, 60)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(screen.contains("Mobile PWA setup"));
        assert!(screen.contains("notes.example.test"));
        assert!(!screen.contains("writable ticket/with secrets"));
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
        assert!(command_screen.contains("http://127.0.0.1:9464/setup"));
        assert!(command_screen.contains("Workspace ID: workspace123"));
        assert!(command_screen.contains("/var/lib/xo-syncd/operator.token"));
        assert!(command_screen.contains("user-unit installer"));
        assert!(command_screen.contains("<writable ticket hidden>"));
        assert_eq!(
            user_syncd_install_command("ticket'with-sensitive-content"),
            "curl -fsSL https://xo.exokephalos.dev/install.sh | XO_SYNC_TICKET='ticket'\"'\"'with-sensitive-content' bash"
        );
        assert!(!command_screen.contains("xo-admin import-ticket"));
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
        let main_choices = app.main_view_choices();
        assert_eq!(
            main_choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.navigation_key.as_str()))
                .collect::<Vec<_>>(),
            vec![("Notes", "n"), ("News", "e"), ("Books", "b")]
        );
        assert!(app.choose_main_view_key('b'));
        assert_eq!(app.active_view, "books");
        assert!(app.active_subview.is_none());

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
            "2026-07-22T10:00:00+00:00",
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
        assert!(preview.contains("created: 2026-07-22T10:00:00+00:00"));
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

        app.mode = Mode::CreateEncryptedTitle;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let encrypted_screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(encrypted_screen.contains("New encrypted item title"));
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
        app.notes[0].frontmatter.insert(
            "created".into(),
            FrontmatterValue::String("2026-01-02T03:04:05+00:00".into()),
        );
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
            OsStr::new(
                "printf '%s' '---\nid: replaced\ntitle: Changed encrypted title\ntype: note\ntags: []\n---\nchanged' > \"$1\"",
            ),
            OsStr::new("_"),
        ];
        let note = app
            .edit_encrypted_with("correct", OsStr::new("sh"), &args)
            .unwrap();
        assert!(app.decrypted_preview.is_none());
        assert_eq!(
            encryption::decrypt("note001", "correct", &note.body).unwrap(),
            "changed"
        );
        assert_eq!(note.id.as_str(), "note001");
        assert_eq!(
            note.frontmatter.get("id"),
            Some(&FrontmatterValue::String("note001".into()))
        );
        assert_eq!(
            note.frontmatter.get("title"),
            Some(&FrontmatterValue::String("Changed encrypted title".into()))
        );
    }

    #[test]
    fn external_editor_may_atomically_replace_the_temporary_file() {
        let args = [
            OsStr::new("-c"),
            OsStr::new(
                "case \"$1\" in *.xo.md) ;; *) exit 9;; esac; \
                 printf changed > \"$1.next\"; mv \"$1.next\" \"$1\"",
            ),
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
    fn horizontal_focus_moves_spatially_across_visible_panes() {
        let mut app = fixture();
        app.pane = Pane::Tags;

        app.focus_left();
        assert_eq!(app.pane, Pane::Tags);
        app.focus_right();
        assert_eq!(app.pane, Pane::Notes);
        app.focus_right();
        assert_eq!(app.pane, Pane::Preview);
        app.focus_right();
        assert_eq!(app.pane, Pane::Preview);
        app.focus_left();
        assert_eq!(app.pane, Pane::Notes);
        app.focus_left();
        assert_eq!(app.pane, Pane::Tags);

        app.toggle_tags_visible();
        assert_eq!(app.pane, Pane::Notes);
        app.focus_left();
        assert_eq!(app.pane, Pane::Notes);
        app.focus_right();
        assert_eq!(app.pane, Pane::Preview);
        app.focus_left();
        assert_eq!(app.pane, Pane::Notes);
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
