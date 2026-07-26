//! Sandboxed Steel configuration loader.
//!
//! `xo.scm` evaluates to a workspace descriptor. Optional `modules/**/*.scm`
//! files use `(workspace-module ...)`; their views, actions, templates, and
//! grants are merged in lexical path order.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;
use thiserror::Error;

use crate::behavior::{
    ActionDescriptor, ActionEffect, BehaviorError, Capability, Predicate, SubviewDescriptor,
    TemplateDescriptor, ViewDescriptor, WorkspaceBehavior,
};
use crate::domain::FrontmatterValue;

pub const MAX_CONFIG_BYTES: usize = 1_048_576;

#[derive(Debug, Error)]
pub enum SteelConfigError {
    #[error("configuration exceeds the {MAX_CONFIG_BYTES}-byte limit")]
    TooLarge,
    #[error("invalid configuration path: {0}")]
    InvalidPath(String),
    #[error("Steel evaluation failed: {0}")]
    Evaluation(String),
    #[error("configuration must contain one declarative workspace-config/workspace-module form")]
    InvalidResult,
    #[error("invalid native xo configuration at byte {offset}: {message}")]
    NativeConfig { offset: usize, message: String },
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
    _now: &str,
) -> Result<WorkspaceBehavior, SteelConfigError> {
    NativeWorkspaceParser::new(source, constructor).parse()
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

#[derive(Clone, Debug)]
enum NativeForm {
    List(Vec<Self>),
    String(String),
    Symbol(String),
}

struct NativeWorkspaceParser<'a> {
    source: &'a str,
    constructor: &'a str,
    position: usize,
}

impl<'a> NativeWorkspaceParser<'a> {
    const fn new(source: &'a str, constructor: &'a str) -> Self {
        Self {
            source,
            constructor,
            position: 0,
        }
    }

    fn parse(mut self) -> Result<WorkspaceBehavior, SteelConfigError> {
        if self.source.len() > MAX_CONFIG_BYTES {
            return Err(SteelConfigError::TooLarge);
        }
        let root = self.form()?;
        self.skip_ignored();
        if self.position != self.source.len() {
            return self.error("unexpected trailing form");
        }
        workspace_behavior(&root, self.constructor)
    }

    fn form(&mut self) -> Result<NativeForm, SteelConfigError> {
        self.skip_ignored();
        match self.peek() {
            Some('(') => {
                self.bump();
                let mut values = Vec::new();
                loop {
                    self.skip_ignored();
                    if self.peek() == Some(')') {
                        self.bump();
                        return Ok(NativeForm::List(values));
                    }
                    if self.peek().is_none() {
                        return self.error("unterminated list");
                    }
                    values.push(self.form()?);
                }
            }
            Some('"') => self.string().map(NativeForm::String),
            Some(')') => self.error("unexpected )"),
            Some(_) => self.token().map(NativeForm::Symbol),
            None => self.error("expected form"),
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

fn workspace_behavior(
    form: &NativeForm,
    constructor: &str,
) -> Result<WorkspaceBehavior, SteelConfigError> {
    let root = constructor_args(form, constructor)?;
    let fields = native_fields(
        root,
        &[
            "schema",
            "default-view",
            "query-limit",
            "views",
            "actions",
            "templates",
            "capability-grants",
        ],
        constructor,
    )?;
    let mut behavior = WorkspaceBehavior {
        schema: optional_u16(&fields, "schema")?.unwrap_or(crate::behavior::BEHAVIOR_SCHEMA),
        default_view: optional_string(&fields, "default-view")?.unwrap_or_else(|| "all".to_owned()),
        query_limit: optional_usize(&fields, "query-limit")?
            .unwrap_or(crate::behavior::DEFAULT_QUERY_LIMIT),
        ..WorkspaceBehavior::default()
    };
    if let Some(forms) = fields.get("views") {
        behavior.views = forms.iter().map(parse_view).collect::<Result<_, _>>()?;
    }
    if let Some(forms) = fields.get("actions") {
        behavior.actions = forms.iter().map(parse_action).collect::<Result<_, _>>()?;
    }
    if let Some(forms) = fields.get("templates") {
        behavior.templates = forms.iter().map(parse_template).collect::<Result<_, _>>()?;
    }
    if let Some(forms) = fields.get("capability-grants") {
        for form in *forms {
            let args = constructor_args(form, "grant")?;
            let grant = native_fields(args, &["action", "capabilities"], "grant")?;
            let action = required_string(&grant, "action")?;
            let capabilities = grant
                .get("capabilities")
                .ok_or_else(|| native_error("grant is missing capabilities"))?
                .iter()
                .map(parse_capability)
                .collect::<Result<_, _>>()?;
            if behavior
                .capability_grants
                .insert(action.clone(), capabilities)
                .is_some()
            {
                return Err(native_error(format!(
                    "duplicate capability grant for {action}"
                )));
            }
        }
    }
    behavior.validate()?;
    Ok(behavior)
}

fn parse_view(form: &NativeForm) -> Result<ViewDescriptor, SteelConfigError> {
    let args = constructor_args(form, "view")?;
    let fields = native_fields(
        args,
        &[
            "id",
            "name",
            "key",
            "show-tags",
            "title-field",
            "subtitle-field",
            "sort-field",
            "descending",
            "preview",
            "predicate",
            "subviews",
        ],
        "view",
    )?;
    Ok(ViewDescriptor {
        id: required_string(&fields, "id")?,
        name: optional_string(&fields, "name")?.unwrap_or_default(),
        key: optional_nullable_string(&fields, "key")?,
        show_tags: optional_bool(&fields, "show-tags")?.unwrap_or(false),
        title_field: optional_string(&fields, "title-field")?.unwrap_or_else(|| "title".to_owned()),
        subtitle_field: optional_nullable_string(&fields, "subtitle-field")?,
        sort_field: optional_nullable_string(&fields, "sort-field")?,
        descending: optional_bool(&fields, "descending")?.unwrap_or(false),
        preview: optional_nullable_string(&fields, "preview")?,
        predicate: optional_predicate(&fields)?.unwrap_or_default(),
        subviews: fields.get("subviews").map_or(Ok(Vec::new()), |forms| {
            forms
                .iter()
                .map(parse_subview)
                .collect::<Result<Vec<_>, _>>()
        })?,
    })
}

fn parse_subview(form: &NativeForm) -> Result<SubviewDescriptor, SteelConfigError> {
    let args = constructor_args(form, "subview")?;
    let fields = native_fields(args, &["id", "name", "predicate"], "subview")?;
    Ok(SubviewDescriptor {
        id: required_string(&fields, "id")?,
        name: optional_string(&fields, "name")?.unwrap_or_default(),
        predicate: optional_predicate(&fields)?.unwrap_or_default(),
    })
}

fn parse_action(form: &NativeForm) -> Result<ActionDescriptor, SteelConfigError> {
    let args = constructor_args(form, "action")?;
    let fields = native_fields(
        args,
        &["id", "description", "predicate", "effects"],
        "action",
    )?;
    Ok(ActionDescriptor {
        id: required_string(&fields, "id")?,
        description: optional_string(&fields, "description")?.unwrap_or_default(),
        predicate: optional_predicate(&fields)?.unwrap_or_default(),
        effects: fields.get("effects").map_or(Ok(Vec::new()), |forms| {
            forms
                .iter()
                .map(parse_effect)
                .collect::<Result<Vec<_>, _>>()
        })?,
    })
}

fn parse_template(form: &NativeForm) -> Result<TemplateDescriptor, SteelConfigError> {
    let args = constructor_args(form, "template")?;
    let fields = native_fields(args, &["id", "path", "content"], "template")?;
    Ok(TemplateDescriptor {
        id: required_string(&fields, "id")?,
        path: required_string(&fields, "path")?,
        content: required_string(&fields, "content")?,
    })
}

fn parse_predicate(form: &NativeForm) -> Result<Predicate, SteelConfigError> {
    let (name, args) = native_call(form)?;
    match (name, args) {
        ("always", []) => Ok(Predicate::Always),
        ("field-equals", [field, value]) => Ok(Predicate::FieldEquals {
            field: native_string(field, "field-equals field")?,
            value: native_string(value, "field-equals value")?,
        }),
        ("has-tag", [tag]) => Ok(Predicate::HasTag {
            tag: native_string(tag, "has-tag tag")?,
        }),
        ("not", [predicate]) => Ok(Predicate::Not {
            predicate: Box::new(parse_predicate(predicate)?),
        }),
        ("all", predicates) => Ok(Predicate::All {
            predicates: predicates
                .iter()
                .map(parse_predicate)
                .collect::<Result<_, _>>()?,
        }),
        ("any", predicates) => Ok(Predicate::Any {
            predicates: predicates
                .iter()
                .map(parse_predicate)
                .collect::<Result<_, _>>()?,
        }),
        _ => Err(native_error(format!("invalid predicate {name}"))),
    }
}

fn parse_effect(form: &NativeForm) -> Result<ActionEffect, SteelConfigError> {
    let (name, args) = native_call(form)?;
    match (name, args) {
        ("add-tag", [tag]) => Ok(ActionEffect::AddTag {
            tag: native_string(tag, "add-tag tag")?,
        }),
        ("remove-tag", [tag]) => Ok(ActionEffect::RemoveTag {
            tag: native_string(tag, "remove-tag tag")?,
        }),
        ("set-field", [field, value]) => Ok(ActionEffect::SetField {
            field: native_string(field, "set-field field")?,
            value: parse_frontmatter_value(value)?,
        }),
        ("append-body", [text]) => Ok(ActionEffect::AppendBody {
            text: native_string(text, "append-body text")?,
        }),
        _ => Err(native_error(format!("invalid action effect {name}"))),
    }
}

fn parse_frontmatter_value(form: &NativeForm) -> Result<FrontmatterValue, SteelConfigError> {
    let (name, args) = native_call(form)?;
    match (name, args) {
        ("null", []) => Ok(FrontmatterValue::Null),
        ("bool", [value]) => Ok(FrontmatterValue::Bool(native_bool(value, "bool")?)),
        ("integer", [value]) => Ok(FrontmatterValue::Integer(native_integer(value, "integer")?)),
        ("float", [value]) => {
            let value = native_symbol(value, "float")?
                .parse::<f64>()
                .map_err(|_| native_error("float requires a finite number"))?;
            if !value.is_finite() {
                return Err(native_error("float requires a finite number"));
            }
            Ok(FrontmatterValue::Float(value))
        }
        ("string", [value]) => Ok(FrontmatterValue::String(native_string(value, "string")?)),
        ("sequence", values) => Ok(FrontmatterValue::Sequence(
            values
                .iter()
                .map(parse_frontmatter_value)
                .collect::<Result<_, _>>()?,
        )),
        ("mapping", entries) => {
            let mut values = BTreeMap::new();
            for entry in entries {
                let (entry_name, entry_args) = native_call(entry)?;
                let [key, value] = entry_args else {
                    return Err(native_error("mapping entry requires a key and value"));
                };
                if entry_name != "entry" {
                    return Err(native_error("mapping accepts only entry forms"));
                }
                let key = native_string(key, "mapping key")?;
                if values
                    .insert(key.clone(), parse_frontmatter_value(value)?)
                    .is_some()
                {
                    return Err(native_error(format!("duplicate mapping key {key}")));
                }
            }
            Ok(FrontmatterValue::Mapping(values))
        }
        _ => Err(native_error(format!("invalid frontmatter value {name}"))),
    }
}

fn parse_capability(form: &NativeForm) -> Result<Capability, SteelConfigError> {
    match native_symbol(form, "capability")? {
        "mutate-note" => Ok(Capability::MutateNote),
        value => Err(native_error(format!("unknown capability {value}"))),
    }
}

fn native_fields<'a>(
    forms: &'a [NativeForm],
    allowed: &[&str],
    context: &str,
) -> Result<BTreeMap<&'a str, &'a [NativeForm]>, SteelConfigError> {
    let mut fields = BTreeMap::new();
    for form in forms {
        let (name, args) = native_call(form)?;
        if !allowed.contains(&name) {
            return Err(native_error(format!("unknown {context} field {name}")));
        }
        if fields.insert(name, args).is_some() {
            return Err(native_error(format!("duplicate {context} field {name}")));
        }
    }
    Ok(fields)
}

fn constructor_args<'a>(
    form: &'a NativeForm,
    expected: &str,
) -> Result<&'a [NativeForm], SteelConfigError> {
    let (name, args) = native_call(form)?;
    if name == expected {
        Ok(args)
    } else {
        Err(native_error(format!("expected {expected}, found {name}")))
    }
}

fn native_call(form: &NativeForm) -> Result<(&str, &[NativeForm]), SteelConfigError> {
    let NativeForm::List(values) = form else {
        return Err(native_error("expected list"));
    };
    let Some((head, args)) = values.split_first() else {
        return Err(native_error("empty list is not a declarative form"));
    };
    Ok((native_symbol(head, "form name")?, args))
}

fn native_symbol<'a>(form: &'a NativeForm, context: &str) -> Result<&'a str, SteelConfigError> {
    if let NativeForm::Symbol(value) = form {
        Ok(value)
    } else {
        Err(native_error(format!("{context} must be an identifier")))
    }
}

fn native_string(form: &NativeForm, context: &str) -> Result<String, SteelConfigError> {
    if let NativeForm::String(value) = form {
        Ok(value.clone())
    } else {
        Err(native_error(format!("{context} must be a string")))
    }
}

fn native_bool(form: &NativeForm, context: &str) -> Result<bool, SteelConfigError> {
    match native_symbol(form, context)? {
        "#t" => Ok(true),
        "#f" => Ok(false),
        _ => Err(native_error(format!("{context} must be #t or #f"))),
    }
}

fn native_integer(form: &NativeForm, context: &str) -> Result<i64, SteelConfigError> {
    native_symbol(form, context)?
        .parse()
        .map_err(|_| native_error(format!("{context} must be an integer")))
}

fn single_field<'a>(
    fields: &BTreeMap<&str, &'a [NativeForm]>,
    name: &str,
) -> Result<Option<&'a NativeForm>, SteelConfigError> {
    fields
        .get(name)
        .map(|values| {
            let [value] = *values else {
                return Err(native_error(format!("{name} requires exactly one value")));
            };
            Ok(value)
        })
        .transpose()
}

fn required_string(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<String, SteelConfigError> {
    optional_string(fields, name)?.ok_or_else(|| native_error(format!("missing field {name}")))
}

fn optional_string(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<Option<String>, SteelConfigError> {
    single_field(fields, name)?
        .map(|value| native_string(value, name))
        .transpose()
}

fn optional_nullable_string(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<Option<String>, SteelConfigError> {
    match single_field(fields, name)? {
        None => Ok(None),
        Some(NativeForm::Symbol(value)) if value == "#f" => Ok(None),
        Some(value) => native_string(value, name).map(Some),
    }
}

fn optional_bool(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<Option<bool>, SteelConfigError> {
    single_field(fields, name)?
        .map(|value| native_bool(value, name))
        .transpose()
}

fn optional_u16(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<Option<u16>, SteelConfigError> {
    single_field(fields, name)?
        .map(|value| {
            native_symbol(value, name)?
                .parse()
                .map_err(|_| native_error(format!("{name} must be an unsigned 16-bit integer")))
        })
        .transpose()
}

fn optional_usize(
    fields: &BTreeMap<&str, &[NativeForm]>,
    name: &str,
) -> Result<Option<usize>, SteelConfigError> {
    single_field(fields, name)?
        .map(|value| {
            native_symbol(value, name)?
                .parse()
                .map_err(|_| native_error(format!("{name} must be a non-negative integer")))
        })
        .transpose()
}

fn optional_predicate(
    fields: &BTreeMap<&str, &[NativeForm]>,
) -> Result<Option<Predicate>, SteelConfigError> {
    single_field(fields, "predicate")?
        .map(parse_predicate)
        .transpose()
}

fn native_error(message: impl Into<String>) -> SteelConfigError {
    SteelConfigError::NativeConfig {
        offset: 0,
        message: message.into(),
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
    path == "xo.scm" || valid_module_path(path)
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
    let constructor = if module {
        "workspace-module"
    } else {
        "workspace-config"
    };
    let mut output = format!(
        "({constructor}\n  (schema {})\n  (default-view {})\n  (query-limit {})",
        behavior.schema,
        steel_string(&behavior.default_view),
        behavior.query_limit
    );
    output.push_str("\n  (views");
    for view in &behavior.views {
        output.push_str(&encode_view(view, 4));
    }
    output.push(')');
    output.push_str("\n  (actions");
    for action in &behavior.actions {
        output.push_str(&encode_action(action, 4));
    }
    output.push(')');
    output.push_str("\n  (templates");
    for template in &behavior.templates {
        write!(
            output,
            "\n    (template\n      (id {})\n      (path {})\n      (content {}))",
            steel_string(&template.id),
            steel_string(&template.path),
            steel_string(&template.content)
        )
        .expect("writing to a String cannot fail");
    }
    output.push(')');
    output.push_str("\n  (capability-grants");
    for (action, capabilities) in &behavior.capability_grants {
        write!(
            output,
            "\n    (grant\n      (action {})\n      (capabilities",
            steel_string(action)
        )
        .expect("writing to a String cannot fail");
        for capability in capabilities {
            write!(output, " {}", encode_capability(*capability))
                .expect("writing to a String cannot fail");
        }
        output.push_str("))");
    }
    output.push_str("))\n");
    output
}

fn encode_view(view: &ViewDescriptor, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    let field = " ".repeat(indent + 2);
    let mut output = format!(
        "\n{prefix}(view\n{field}(id {})\n{field}(name {})\n{field}(key {})\n{field}(show-tags {})\n{field}(title-field {})\n{field}(subtitle-field {})\n{field}(sort-field {})\n{field}(descending {})\n{field}(preview {})\n{field}(predicate {})\n{field}(subviews",
        steel_string(&view.id),
        steel_string(&view.name),
        encode_optional_string(view.key.as_deref()),
        encode_bool(view.show_tags),
        steel_string(&view.title_field),
        encode_optional_string(view.subtitle_field.as_deref()),
        encode_optional_string(view.sort_field.as_deref()),
        encode_bool(view.descending),
        encode_optional_string(view.preview.as_deref()),
        encode_predicate(&view.predicate),
    );
    for subview in &view.subviews {
        write!(
            output,
            "\n{}(subview\n{}(id {})\n{}(name {})\n{}(predicate {}))",
            " ".repeat(indent + 4),
            " ".repeat(indent + 6),
            steel_string(&subview.id),
            " ".repeat(indent + 6),
            steel_string(&subview.name),
            " ".repeat(indent + 6),
            encode_predicate(&subview.predicate)
        )
        .expect("writing to a String cannot fail");
    }
    output.push_str("))");
    output
}

fn encode_action(action: &ActionDescriptor, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    let field = " ".repeat(indent + 2);
    let mut output = format!(
        "\n{prefix}(action\n{field}(id {})\n{field}(description {})\n{field}(predicate {})\n{field}(effects",
        steel_string(&action.id),
        steel_string(&action.description),
        encode_predicate(&action.predicate)
    );
    for effect in &action.effects {
        write!(
            output,
            "\n{}{}",
            " ".repeat(indent + 4),
            encode_effect(effect)
        )
        .expect("writing to a String cannot fail");
    }
    output.push_str("))");
    output
}

fn encode_predicate(predicate: &Predicate) -> String {
    match predicate {
        Predicate::Always => "(always)".to_owned(),
        Predicate::FieldEquals { field, value } => format!(
            "(field-equals {} {})",
            steel_string(field),
            steel_string(value)
        ),
        Predicate::HasTag { tag } => format!("(has-tag {})", steel_string(tag)),
        Predicate::Not { predicate } => format!("(not {})", encode_predicate(predicate)),
        Predicate::All { predicates } => encode_predicate_list("all", predicates),
        Predicate::Any { predicates } => encode_predicate_list("any", predicates),
    }
}

fn encode_effect(effect: &ActionEffect) -> String {
    match effect {
        ActionEffect::AddTag { tag } => format!("(add-tag {})", steel_string(tag)),
        ActionEffect::RemoveTag { tag } => format!("(remove-tag {})", steel_string(tag)),
        ActionEffect::SetField { field, value } => format!(
            "(set-field {} {})",
            steel_string(field),
            encode_frontmatter_value(value)
        ),
        ActionEffect::AppendBody { text } => format!("(append-body {})", steel_string(text)),
    }
}

fn encode_frontmatter_value(value: &FrontmatterValue) -> String {
    match value {
        FrontmatterValue::Null => "(null)".to_owned(),
        FrontmatterValue::Bool(value) => format!("(bool {})", encode_bool(*value)),
        FrontmatterValue::Integer(value) => format!("(integer {value})"),
        FrontmatterValue::Float(value) => format!("(float {value})"),
        FrontmatterValue::String(value) => format!("(string {})", steel_string(value)),
        FrontmatterValue::Sequence(values) => {
            let mut output = "(sequence".to_owned();
            for value in values {
                write!(output, " {}", encode_frontmatter_value(value))
                    .expect("writing to a String cannot fail");
            }
            output.push(')');
            output
        }
        FrontmatterValue::Mapping(values) => {
            let mut output = "(mapping".to_owned();
            for (key, value) in values {
                write!(
                    output,
                    " (entry {} {})",
                    steel_string(key),
                    encode_frontmatter_value(value)
                )
                .expect("writing to a String cannot fail");
            }
            output.push(')');
            output
        }
    }
}

fn encode_predicate_list(name: &str, predicates: &[Predicate]) -> String {
    let mut output = format!("({name}");
    for predicate in predicates {
        write!(output, " {}", encode_predicate(predicate))
            .expect("writing to a String cannot fail");
    }
    output.push(')');
    output
}

const fn encode_bool(value: bool) -> &'static str {
    if value { "#t" } else { "#f" }
}

fn encode_optional_string(value: Option<&str>) -> String {
    value.map_or_else(|| "#f".to_owned(), steel_string)
}

fn encode_capability(capability: Capability) -> &'static str {
    match capability {
        Capability::MutateNote => "mutate-note",
    }
}

fn steel_string(value: &str) -> String {
    serde_json::to_string(value).expect("Steel strings are serializable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn loads_and_merges_native_modules() {
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
    fn native_workspace_config_round_trips_every_descriptor_field() {
        let behavior = WorkspaceBehavior {
            schema: 1,
            default_view: "notes".into(),
            views: vec![ViewDescriptor {
                id: "notes".into(),
                name: "Notes".into(),
                key: Some("n".into()),
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: Some("type".into()),
                sort_field: Some("created".into()),
                descending: true,
                preview: Some("{{body}}".into()),
                predicate: Predicate::All {
                    predicates: vec![
                        Predicate::FieldEquals {
                            field: "type".into(),
                            value: "note".into(),
                        },
                        Predicate::Not {
                            predicate: Box::new(Predicate::HasTag {
                                tag: "archived".into(),
                            }),
                        },
                    ],
                },
                subviews: vec![SubviewDescriptor {
                    id: "important".into(),
                    name: "Important".into(),
                    predicate: Predicate::Any {
                        predicates: vec![
                            Predicate::HasTag {
                                tag: "important".into(),
                            },
                            Predicate::Always,
                        ],
                    },
                }],
            }],
            actions: vec![ActionDescriptor {
                id: "finish".into(),
                description: "Finish reading".into(),
                predicate: Predicate::HasTag {
                    tag: "reading".into(),
                },
                effects: vec![
                    ActionEffect::AddTag { tag: "read".into() },
                    ActionEffect::RemoveTag {
                        tag: "reading".into(),
                    },
                    ActionEffect::SetField {
                        field: "metadata".into(),
                        value: FrontmatterValue::Mapping(BTreeMap::from([
                            ("complete".into(), FrontmatterValue::Bool(true)),
                            (
                                "values".into(),
                                FrontmatterValue::Sequence(vec![
                                    FrontmatterValue::Null,
                                    FrontmatterValue::Integer(2),
                                    FrontmatterValue::Float(3.5),
                                    FrontmatterValue::String("done".into()),
                                ]),
                            ),
                        ])),
                    },
                    ActionEffect::AppendBody {
                        text: "\nFinished.\n".into(),
                    },
                ],
            }],
            templates: vec![TemplateDescriptor {
                id: "daily".into(),
                path: "daily/{{date}}.md".into(),
                content: "---\ntitle: {{date}}\n---\n".into(),
            }],
            capability_grants: BTreeMap::from([(
                "finish".into(),
                BTreeSet::from([Capability::MutateNote]),
            )]),
            query_limit: 42,
        };

        let source = encode_config(&behavior, false);
        assert!(source.starts_with("(workspace-config\n  (schema 1)"));
        assert!(source.contains("(field-equals \"type\" \"note\")"));
        assert!(!source.starts_with("(workspace-config \""));
        let loaded = SteelWorkspace::load(&source, &BTreeMap::new(), "fixed").unwrap();
        assert_eq!(loaded, behavior);

        assert!(
            SteelWorkspace::load(
                "(workspace-config \"{\\\"query_limit\\\":42}\")",
                &BTreeMap::new(),
                "fixed"
            )
            .is_err()
        );
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
        for probe in [
            "(workspace-config (views (open-input-file \"/etc/passwd\")))",
            "(workspace-config (views) (query-limit (current-second)))",
            "(workspace-config (views)) (env-var \"HOME\")",
        ] {
            assert!(
                SteelWorkspace::load(probe, &BTreeMap::new(), "fixed-time").is_err(),
                "native adapter exposed {probe}"
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
    fn migrated_example_has_equivalent_native_behavior() {
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
