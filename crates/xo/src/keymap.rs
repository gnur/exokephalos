use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub const ACTION_NAMES: &[&str] = &[
    "action_picker",
    "create_encrypted_item",
    "create_item",
    "clear_search",
    "cursor_down",
    "cursor_up",
    "delete_item",
    "edit_item",
    "edit_workspace_config",
    "focus_column_left",
    "focus_column_right",
    "focus_subview_next",
    "focus_subview_previous",
    "goto_view",
    "open_conflicts",
    "open_goto",
    "open_item_actions",
    "open_peers",
    "open_search",
    "open_sync_status",
    "open_view_picker",
    "quit",
    "refresh_sync",
    "restore_item",
    "retry_operation",
    "reverse_sort",
    "toggle_tag",
    "toggle_tags_column",
    "unlock_preview",
];

/// Short action names accepted by both keys.scm and the action picker.
pub const ACTION_ALIASES: &[(&str, &str)] = &[
    ("c", "create_item"),
    ("d", "delete_item"),
    ("e", "edit_item"),
    ("g", "open_view_picker"),
    ("h", "focus_column_left"),
    ("j", "cursor_down"),
    ("k", "cursor_up"),
    ("l", "focus_column_right"),
    ("p", "open_peers"),
    ("q", "quit"),
    ("u", "restore_item"),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionCall {
    pub name: String,
    pub arguments: Vec<String>,
}

impl ActionCall {
    pub fn parse(value: &str) -> Result<Self> {
        let mut parts = value.split_whitespace();
        let entered_name = parts.next().context("action name is required")?;
        let name = ACTION_ALIASES
            .iter()
            .find_map(|(alias, name)| (*alias == entered_name).then_some(*name))
            .unwrap_or(entered_name)
            .to_owned();
        if !ACTION_NAMES.contains(&name.as_str()) {
            bail!("unknown TUI action {entered_name:?}");
        }
        let arguments = parts.map(str::to_owned).collect::<Vec<_>>();
        if name == "goto_view" && arguments.len() != 1 {
            bail!("goto_view requires one view or view/subview argument");
        }
        if name != "goto_view" && !arguments.is_empty() {
            bail!("{name} does not accept arguments");
        }
        Ok(Self { name, arguments })
    }

    #[must_use]
    pub fn display(&self) -> String {
        std::iter::once(self.name.as_str())
            .chain(self.arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyMap {
    bindings: BTreeMap<String, ActionCall>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::from_source(DEFAULT_KEYS).expect("built-in keymap must be valid")
    }
}

impl KeyMap {
    pub fn load_or_create(path: &Path) -> Result<(Self, String)> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(path, DEFAULT_KEYS)
                .with_context(|| format!("write default keymap {}", path.display()))?;
        }
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("read keymap {}", path.display()))?;
        Ok((Self::from_source(&source)?, source))
    }

    pub fn from_source(source: &str) -> Result<Self> {
        let forms = Parser::new(source).parse_all()?;
        let [Form::List(root)] = forms.as_slice() else {
            bail!("keys.scm must contain one (keys ...) form");
        };
        if atom(root.first()) != Some("keys") {
            bail!("keys.scm must start with (keys ...)");
        }
        let mut bindings = BTreeMap::new();
        for form in &root[1..] {
            let Form::List(values) = form else {
                bail!("key bindings must use (bind key action)");
            };
            if atom(values.first()) != Some("bind") || values.len() != 3 {
                bail!("key bindings must use (bind key action)");
            }
            let key = scalar(&values[1]).context("binding key must be a string or atom")?;
            let action = action(&values[2])?;
            let key = canonical_key_name(&key)?;
            if bindings.insert(key.clone(), action).is_some() {
                bail!("duplicate key binding {key:?}");
            }
        }
        Ok(Self { bindings })
    }

    #[must_use]
    pub fn action_for(&self, event: KeyEvent) -> Option<&ActionCall> {
        self.bindings.get(&event_key_name(event)?)
    }

    #[must_use]
    pub fn keys_for(&self, action: &str) -> Vec<&str> {
        self.bindings
            .iter()
            .filter_map(|(key, call)| (call.name == action).then_some(key.as_str()))
            .collect()
    }

    #[must_use]
    pub fn footer_key(&self, action: &str) -> String {
        self.keys_for(action)
            .first()
            .copied()
            .unwrap_or("—")
            .to_owned()
    }
}

pub const DEFAULT_KEYS: &str = r#"; Hot-reloaded xo TUI bindings.
(keys
  (bind "j" cursor_down)
  (bind "down" cursor_down)
  (bind "k" cursor_up)
  (bind "up" cursor_up)
  (bind "h" focus_column_left)
  (bind "left" focus_column_left)
  (bind "l" focus_column_right)
  (bind "right" focus_column_right)
  (bind "tab" focus_subview_next)
  (bind "backtab" focus_subview_previous)
  (bind "space" toggle_tag)
  (bind "esc" clear_search)
  (bind "enter" edit_item)
  (bind "e" edit_item)
  (bind "/" open_search)
  (bind "d" delete_item)
  (bind "g" open_view_picker)
  (bind ":" action_picker)
  (bind "c" create_item)
  (bind "C" create_encrypted_item)
  (bind "u" restore_item)
  (bind "q" q))
"#;

fn action(form: &Form) -> Result<ActionCall> {
    match form {
        Form::Atom(_) | Form::String(_) => ActionCall::parse(&scalar(form).unwrap_or_default()),
        Form::List(values) => {
            let Some(name) = values.first().and_then(scalar) else {
                bail!("action call cannot be empty");
            };
            let arguments = values[1..]
                .iter()
                .map(|value| scalar(value).context("action arguments must be strings or atoms"))
                .collect::<Result<Vec<_>>>()?;
            ActionCall::parse(
                &std::iter::once(name)
                    .chain(arguments)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
    }
}

fn event_key_name(event: KeyEvent) -> Option<String> {
    let base = match event.code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(value) => value.to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "backtab".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::F(number) => format!("f{number}"),
        _ => return None,
    };
    let mut prefixes = Vec::new();
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        prefixes.push("ctrl");
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        prefixes.push("alt");
    }
    if event.modifiers.contains(KeyModifiers::SHIFT) && !matches!(event.code, KeyCode::Char(_)) {
        prefixes.push("shift");
    }
    prefixes.push(&base);
    Some(prefixes.join("+"))
}

fn canonical_key_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("binding key cannot be empty");
    }
    Ok(match value {
        " " => "space".to_owned(),
        "<tab>" => "tab".to_owned(),
        "<backtab>" => "backtab".to_owned(),
        "<enter>" => "enter".to_owned(),
        other => other.to_owned(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Form {
    Atom(String),
    String(String),
    List(Vec<Form>),
}

fn atom(form: Option<&Form>) -> Option<&str> {
    match form? {
        Form::Atom(value) => Some(value),
        _ => None,
    }
}

fn scalar(form: &Form) -> Option<String> {
    match form {
        Form::Atom(value) | Form::String(value) => Some(value.clone()),
        Form::List(_) => None,
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            offset: 0,
        }
    }

    fn parse_all(mut self) -> Result<Vec<Form>> {
        let mut forms = Vec::new();
        self.space();
        while self.offset < self.bytes.len() {
            forms.push(self.form()?);
            self.space();
        }
        Ok(forms)
    }

    fn form(&mut self) -> Result<Form> {
        self.space();
        match self.bytes.get(self.offset).copied() {
            Some(b'(') => {
                self.offset += 1;
                let mut values = Vec::new();
                loop {
                    self.space();
                    match self.bytes.get(self.offset).copied() {
                        Some(b')') => {
                            self.offset += 1;
                            return Ok(Form::List(values));
                        }
                        None => bail!("unterminated list in keys.scm"),
                        _ => values.push(self.form()?),
                    }
                }
            }
            Some(b'"') => self.string(),
            Some(b')') => bail!("unexpected ')' in keys.scm"),
            Some(_) => self.atom(),
            None => bail!("unexpected end of keys.scm"),
        }
    }

    fn string(&mut self) -> Result<Form> {
        let start = self.offset;
        self.offset += 1;
        let mut escaped = false;
        while let Some(byte) = self.bytes.get(self.offset).copied() {
            self.offset += 1;
            if byte == b'"' && !escaped {
                let raw = std::str::from_utf8(&self.bytes[start..self.offset])?;
                return Ok(Form::String(serde_json::from_str(raw)?));
            }
            escaped = byte == b'\\' && !escaped;
            if byte != b'\\' {
                escaped = false;
            }
        }
        bail!("unterminated string in keys.scm")
    }

    fn atom(&mut self) -> Result<Form> {
        let start = self.offset;
        while let Some(byte) = self.bytes.get(self.offset).copied() {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b';') {
                break;
            }
            self.offset += 1;
        }
        if start == self.offset {
            bail!("invalid token in keys.scm");
        }
        Ok(Form::Atom(
            std::str::from_utf8(&self.bytes[start..self.offset])?.to_owned(),
        ))
    }

    fn space(&mut self) {
        loop {
            while self
                .bytes
                .get(self.offset)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.offset += 1;
            }
            if self.bytes.get(self.offset) == Some(&b';') {
                while self
                    .bytes
                    .get(self.offset)
                    .is_some_and(|byte| *byte != b'\n')
                {
                    self.offset += 1;
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_argument_bindings_parse() {
        let keys = KeyMap::default();
        let event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(keys.action_for(event).unwrap().name, "cursor_down");
        assert_eq!(
            keys.action_for(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
                .unwrap()
                .name,
            "quit"
        );
        assert_eq!(
            keys.action_for(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                .unwrap()
                .name,
            "clear_search"
        );
        assert_eq!(ActionCall::parse("q").unwrap().name, "quit");
        assert_eq!(ActionCall::parse("p").unwrap().name, "open_peers");
        let custom = KeyMap::from_source(
            r#"(keys (bind "b" (goto_view "books/read")) (bind ":" action_picker))"#,
        )
        .unwrap();
        assert_eq!(
            custom
                .action_for(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
                .unwrap()
                .display(),
            "goto_view books/read"
        );
    }

    #[test]
    fn missing_keymap_is_created_and_can_be_reloaded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.scm");
        let (initial, source) = KeyMap::load_or_create(&path).unwrap();
        assert_eq!(initial.footer_key("action_picker"), ":");
        assert_eq!(source, DEFAULT_KEYS);
        std::fs::write(&path, "(keys (bind \";\" action_picker))").unwrap();
        let (reloaded, _) = KeyMap::load_or_create(&path).unwrap();
        assert_eq!(reloaded.footer_key("action_picker"), ";");
    }

    #[test]
    fn invalid_and_duplicate_bindings_are_rejected() {
        assert!(KeyMap::from_source("(keys (bind \"x\" missing))").is_err());
        assert!(KeyMap::from_source("(keys (bind \"x\" quit) (bind \"x\" quit))").is_err());
    }
}
