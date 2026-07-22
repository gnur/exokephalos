//! Sandboxed Steel configuration loader.
//!
//! `exo.scm` evaluates to `(workspace-config "<descriptor-json>")`. Optional
//! `modules/**/*.scm` files use `(workspace-module "<descriptor-json>")`; their
//! views, actions, templates, and grants are merged in lexical path order.

use std::collections::BTreeMap;

use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;
use thiserror::Error;

use crate::behavior::{BehaviorError, WorkspaceBehavior};

pub const MAX_CONFIG_BYTES: usize = 1_048_576;

#[derive(Debug, Error)]
pub enum SteelConfigError {
    #[error("configuration exceeds the {MAX_CONFIG_BYTES}-byte limit")]
    TooLarge,
    #[error("invalid configuration path: {0}")]
    InvalidPath(String),
    #[error("Steel evaluation failed: {0}")]
    Evaluation(String),
    #[error("configuration must return descriptor JSON through workspace-config/workspace-module")]
    InvalidResult,
    #[error("invalid native xo configuration at byte {offset}: {message}")]
    NativeConfig { offset: usize, message: String },
    #[error("invalid behavior descriptor: {0}")]
    Descriptor(#[from] serde_json::Error),
    #[error(transparent)]
    Behavior(#[from] BehaviorError),
}

/// Narrow host adapter. It exposes only pure string/tag helpers and an explicit
/// caller-supplied clock value; the underlying VM is Steel's sandboxed engine.
pub struct SteelWorkspace;

impl SteelWorkspace {
    pub fn load(
        exo_scm: &str,
        modules: &BTreeMap<String, String>,
        deterministic_now: &str,
    ) -> Result<WorkspaceBehavior, SteelConfigError> {
        let mut behavior = evaluate(exo_scm, "workspace-config", deterministic_now)?;
        for (path, source) in modules {
            if !valid_module_path(path) {
                return Err(SteelConfigError::InvalidPath(path.clone()));
            }
            let module = evaluate(source, "workspace-module", deterministic_now)?;
            behavior.views.extend(module.views);
            behavior.actions.extend(module.actions);
            behavior.templates.extend(module.templates);
            behavior.capability_grants.extend(module.capability_grants);
        }
        behavior.validate()?;
        Ok(behavior)
    }
}

fn evaluate(
    source: &str,
    constructor: &str,
    now: &str,
) -> Result<WorkspaceBehavior, SteelConfigError> {
    Ok(serde_json::from_str(&evaluate_descriptor(
        source,
        constructor,
        now,
    )?)?)
}

/// Evaluate one exact descriptor constructor and return its JSON payload.
///
/// The source is admitted only when it consists of a single constructor call
/// with one string literal, preventing ambient Steel capabilities from running.
pub fn evaluate_descriptor(
    source: &str,
    constructor: &str,
    now: &str,
) -> Result<String, SteelConfigError> {
    if source.len() > MAX_CONFIG_BYTES {
        return Err(SteelConfigError::TooLarge);
    }
    if !matches!(constructor, "workspace-config" | "workspace-module") {
        return Err(SteelConfigError::InvalidResult);
    }
    // The descriptor boundary deliberately excludes eval/module loading. This
    // also makes the exact behavior portable to clients that do not embed Steel.
    let trimmed = source.trim();
    let prefix = format!("({constructor} ");
    let literal = trimmed
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(')'))
        .ok_or(SteelConfigError::InvalidResult)?;
    // Parsing the sole argument before VM entry guarantees there is exactly one
    // form and that executable text cannot be smuggled before or after it.
    let expected_json: String =
        serde_json::from_str(literal).map_err(|_| SteelConfigError::InvalidResult)?;
    let mut engine = sandbox(now);
    let values = engine
        .run(source.to_owned())
        .map_err(|error| SteelConfigError::Evaluation(error.to_string()))?;
    let json = match values.last() {
        Some(SteelVal::StringV(value)) => value.to_string(),
        _ => return Err(SteelConfigError::InvalidResult),
    };
    if json != expected_json {
        return Err(SteelConfigError::InvalidResult);
    }
    Ok(json)
}

/// Evaluate the native `~/.config/xo/config.scm` schema.
///
/// Only the five named field forms are admitted. The parsed values are rebuilt
/// into a canonical program before Steel evaluates them, so arbitrary ambient
/// expressions can never execute.
pub fn evaluate_xo_config(source: &str) -> Result<String, SteelConfigError> {
    if source.len() > MAX_CONFIG_BYTES {
        return Err(SteelConfigError::TooLarge);
    }
    let fields = NativeXoParser::new(source).parse()?;
    let canonical = fields.canonical();
    let mut engine = sandbox("fixed");
    let values = engine
        .run(canonical)
        .map_err(|error| SteelConfigError::Evaluation(error.to_string()))?;
    match values.last() {
        Some(SteelVal::StringV(value)) => Ok(value.to_string()),
        _ => Err(SteelConfigError::InvalidResult),
    }
}

fn sandbox(now: &str) -> Engine {
    let mut engine = Engine::new_sandboxed();
    engine
        .register_fn("workspace-config", |json: String| json)
        .register_fn("workspace-module", |json: String| json)
        .register_fn("schema", |value: isize| value.to_string())
        .register_fn("state-dir", |value: String| value)
        .register_fn("workspace", optional_config_value)
        .register_fn("projection", |value: String| value)
        .register_fn(
            "xo-config",
            |schema: String, state_dir: String, workspace: String, projection: String| {
                serde_json::json!({
                    "schema": schema.parse::<u16>().unwrap_or_default(),
                    "state_dir": state_dir,
                    "workspace": (!workspace.is_empty()).then_some(workspace),
                    "projection": projection,
                })
                .to_string()
            },
        )
        .register_fn("exo-has-tag", |tags: String, tag: String| {
            tags.split(',').map(str::trim).any(|value| value == tag)
        })
        .register_fn("exo-add-tag", |tags: String, tag: String| {
            update_tags(&tags, &tag, true)
        })
        .register_fn("exo-remove-tag", |tags: String, tag: String| {
            update_tags(&tags, &tag, false)
        });
    let now = now.to_owned();
    engine.register_fn("exo-now", move || now.clone());
    engine
}

fn optional_config_value(value: SteelVal) -> String {
    match value {
        SteelVal::StringV(value) => value.to_string(),
        _ => String::new(),
    }
}

#[derive(Debug)]
struct NativeXoFields {
    schema: u16,
    state_dir: String,
    workspace: Option<String>,
    projection: String,
}

impl NativeXoFields {
    fn canonical(&self) -> String {
        let string = |value: &str| {
            serde_json::to_string(value).expect("native config strings are serializable")
        };
        let optional = |value: Option<&str>| value.map_or_else(|| "#f".to_owned(), string);
        format!(
            "(xo-config (schema {}) (state-dir {}) (workspace {}) (projection {}))",
            self.schema,
            string(&self.state_dir),
            optional(self.workspace.as_deref()),
            string(&self.projection),
        )
    }
}

struct NativeXoParser<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> NativeXoParser<'a> {
    const fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn parse(mut self) -> Result<NativeXoFields, SteelConfigError> {
        self.expect_char('(')?;
        self.expect_token("xo-config")?;
        let mut schema = None;
        let mut state_dir = None;
        let mut workspace = None;
        let mut projection = None;
        loop {
            self.skip_ignored();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            self.expect_char('(')?;
            let key = self.token()?;
            match key.as_str() {
                "schema" => set_once(&mut schema, self.integer()?, "schema", self.position)?,
                "state-dir" => {
                    set_once(&mut state_dir, self.string()?, "state-dir", self.position)?;
                }
                "workspace" => set_once(
                    &mut workspace,
                    self.optional_string()?,
                    "workspace",
                    self.position,
                )?,
                "projection" => {
                    set_once(&mut projection, self.string()?, "projection", self.position)?;
                }
                _ => return self.error(format!("unknown field {key}")),
            }
            self.expect_char(')')?;
        }
        self.skip_ignored();
        if self.position != self.source.len() {
            return self.error("unexpected trailing form");
        }
        Ok(NativeXoFields {
            schema: required(schema, "schema", self.position)?,
            state_dir: required(state_dir, "state-dir", self.position)?,
            workspace: required(workspace, "workspace", self.position)?,
            projection: required(projection, "projection", self.position)?,
        })
    }

    fn integer(&mut self) -> Result<u16, SteelConfigError> {
        let value = self.token()?;
        value.parse().map_err(|_| SteelConfigError::NativeConfig {
            offset: self.position,
            message: "schema must be an unsigned 16-bit integer".to_owned(),
        })
    }

    fn optional_string(&mut self) -> Result<Option<String>, SteelConfigError> {
        self.skip_ignored();
        if self.peek() == Some('"') {
            self.string().map(Some)
        } else if self.token()? == "#f" {
            Ok(None)
        } else {
            self.error("optional values must be a string or #f")
        }
    }

    fn string(&mut self) -> Result<String, SteelConfigError> {
        self.skip_ignored();
        let start = self.position;
        if self.bump() != Some('"') {
            return self.error("expected string");
        }
        let mut escaped = false;
        while let Some(value) = self.bump() {
            if escaped {
                escaped = false;
            } else if value == '\\' {
                escaped = true;
            } else if value == '"' {
                return serde_json::from_str(&self.source[start..self.position]).map_err(|_| {
                    SteelConfigError::NativeConfig {
                        offset: start,
                        message: "invalid string escape".to_owned(),
                    }
                });
            }
        }
        self.error("unterminated string")
    }

    fn token(&mut self) -> Result<String, SteelConfigError> {
        self.skip_ignored();
        let start = self.position;
        while self
            .peek()
            .is_some_and(|value| !value.is_whitespace() && !matches!(value, '(' | ')' | ';'))
        {
            self.bump();
        }
        if start == self.position {
            return self.error("expected identifier or value");
        }
        Ok(self.source[start..self.position].to_owned())
    }

    fn expect_token(&mut self, expected: &str) -> Result<(), SteelConfigError> {
        let actual = self.token()?;
        if actual == expected {
            Ok(())
        } else {
            self.error(format!("expected {expected}, found {actual}"))
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), SteelConfigError> {
        self.skip_ignored();
        if self.bump() == Some(expected) {
            Ok(())
        } else {
            self.error(format!("expected {expected}"))
        }
    }

    fn skip_ignored(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() != Some(';') {
                break;
            }
            while self.peek().is_some_and(|value| value != '\n') {
                self.bump();
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.position += value.len_utf8();
        Some(value)
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, SteelConfigError> {
        Err(SteelConfigError::NativeConfig {
            offset: self.position,
            message: message.into(),
        })
    }
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    field: &str,
    offset: usize,
) -> Result<(), SteelConfigError> {
    if slot.replace(value).is_some() {
        Err(SteelConfigError::NativeConfig {
            offset,
            message: format!("duplicate field {field}"),
        })
    } else {
        Ok(())
    }
}

fn required<T>(value: Option<T>, field: &str, offset: usize) -> Result<T, SteelConfigError> {
    value.ok_or_else(|| SteelConfigError::NativeConfig {
        offset,
        message: format!("missing field {field}"),
    })
}

fn update_tags(tags: &str, tag: &str, add: bool) -> String {
    let mut values = tags
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    if add {
        values.insert(tag.to_owned());
    } else {
        values.remove(tag);
    }
    values.into_iter().collect::<Vec<_>>().join(",")
}

#[must_use]
pub fn valid_config_path(path: &str) -> bool {
    path == "exo.scm" || valid_module_path(path)
}

fn valid_module_path(path: &str) -> bool {
    path.starts_with("modules/")
        && std::path::Path::new(path)
            .extension()
            .is_some_and(|value| value == "scm")
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[must_use]
pub fn encode_config(behavior: &WorkspaceBehavior, module: bool) -> String {
    let json = serde_json::to_string(behavior).expect("serializable behavior descriptor");
    let literal = serde_json::to_string(&json).expect("serializable Steel string");
    format!(
        "({} {literal})\n",
        if module {
            "workspace-module"
        } else {
            "workspace-config"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::{ActionDescriptor, Predicate, ViewDescriptor};

    #[test]
    fn loads_and_merges_portable_descriptors() {
        let base = WorkspaceBehavior {
            views: vec![view("notes")],
            ..WorkspaceBehavior::default()
        };
        let module = WorkspaceBehavior {
            default_view: "all".into(),
            actions: vec![ActionDescriptor {
                id: "done".into(),
                description: String::new(),
                predicate: Predicate::default(),
                effects: vec![],
            }],
            ..WorkspaceBehavior::default()
        };
        let loaded = SteelWorkspace::load(
            &encode_config(&base, false),
            &BTreeMap::from([(
                "modules/actions/main.scm".into(),
                encode_config(&module, true),
            )]),
            "2026-01-02T03:04:05Z",
        )
        .unwrap();
        assert_eq!(loaded.views[0].id, "notes");
        assert_eq!(loaded.actions[0].id, "done");
    }

    #[test]
    fn sandbox_and_schema_reject_ambient_capabilities() {
        for source in [
            "(open-input-file \"/etc/passwd\")",
            "(env-var \"HOME\")",
            "(command \"id\")",
            "(tcp-connect \"localhost\" 80)",
            "(current-second)",
            "(eval! \"1\")",
        ] {
            assert!(
                SteelWorkspace::load(source, &BTreeMap::new(), "fixed").is_err(),
                "accepted {source}"
            );
        }
        let mut engine = sandbox("fixed-time");
        let value = engine.run("(exo-now)".to_owned()).unwrap();
        assert!(
            matches!(value.last(), Some(SteelVal::StringV(value)) if value.as_str() == "fixed-time")
        );
        for probe in [
            "(workspace-config \"{}\") (open-input-file \"/etc/passwd\")",
            "(workspace-config \"{}\") (env-var \"HOME\")",
            "(workspace-config \"{}\") (command \"id\")",
            "(workspace-config \"{}\") (tcp-connect \"localhost\" 80)",
        ] {
            assert!(
                SteelWorkspace::load(probe, &BTreeMap::new(), "fixed-time").is_err(),
                "adapter exposed {probe}"
            );
        }
        let native_attack = r#"(xo-config
            (schema 1)
            (state-dir (env-var "HOME"))
            (workspace #f)
            (projection "."))"#;
        assert!(evaluate_xo_config(native_attack).is_err());
    }

    #[test]
    fn migrated_example_has_equivalent_predicates_and_executes_as_steel() {
        use crate::behavior::Query;
        use crate::domain::FrontmatterValue;
        let behavior = crate::legacy_config::migrate_fennel(include_str!(
            "../../../oldcodebase/example-repo/exo.fnl"
        ))
        .unwrap();
        let loaded =
            SteelWorkspace::load(&encode_config(&behavior, false), &BTreeMap::new(), "fixed")
                .unwrap();
        let scan = crate::projection::scan_for_import(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../oldcodebase/example-repo"
        )))
        .unwrap();
        assert_eq!(scan.notes.len(), 278);
        for view in &loaded.views {
            let expected = scan
                .notes
                .iter()
                .filter(|note| {
                    let string = |field: &str| match note.frontmatter.get(field) {
                        Some(FrontmatterValue::String(value)) => value.as_str(),
                        _ => "",
                    };
                    let tags = match note.frontmatter.get("tags") {
                        Some(FrontmatterValue::Sequence(values)) => values
                            .iter()
                            .filter_map(|value| {
                                if let FrontmatterValue::String(value) = value {
                                    Some(value.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>(),
                        _ => vec![],
                    };
                    match view.id.as_str() {
                        "books" => ["read", "to-read", "reading", "stopped-reading"]
                            .iter()
                            .any(|tag| tags.contains(tag)),
                        "docs" => string("type") == "doc",
                        "notes" => {
                            string("type") == "note"
                                && !["read", "to-read", "reading", "stopped-reading"]
                                    .iter()
                                    .any(|tag| tags.contains(tag))
                        }
                        "secrets" => string("type") == "secret",
                        "webhooks" => matches!(string("type"), "webhook" | "alert"),
                        _ => false,
                    }
                })
                .count();
            let actual = loaded
                .query(
                    &scan.notes,
                    &Query {
                        view: view.id.clone(),
                        ..Query::default()
                    },
                )
                .unwrap()
                .len();
            assert_eq!(
                actual,
                expected.min(loaded.query_limit),
                "view {} differs",
                view.id
            );
        }
    }

    fn view(id: &str) -> ViewDescriptor {
        ViewDescriptor {
            id: id.into(),
            name: id.into(),
            key: None,
            show_tags: false,
            title_field: "title".into(),
            subtitle_field: None,
            sort_field: None,
            descending: false,
            preview: None,
            predicate: Predicate::default(),
            subviews: vec![],
        }
    }
}
