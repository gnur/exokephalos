//! Sandboxed Steel configuration loader.
//!
//! `xo.scm` evaluates to a workspace descriptor. Optional `modules/**/*.scm`
//! files use `(workspace-module ...)`. Executable `plugins/**/*.scm` files run
//! only for manifest discovery here; action execution uses a fresh sandboxed
//! VM with capability-checked host services.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Deserialize;
use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::interrupt::InterruptHandler;
use steel::steel_vm::register_fn::RegisterFn;
use thiserror::Error;

use crate::behavior::{
    ActionDescriptor, ActionEffect, ActionPlugin, BehaviorError, Capability, Predicate,
    SubviewDescriptor, TemplateDescriptor, ViewDescriptor, WorkspaceBehavior,
};
use crate::domain::FrontmatterValue;

pub const MAX_CONFIG_BYTES: usize = 1_048_576;
const PLUGIN_MANIFEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
    #[error("invalid Steel plugin manifest in {path}: {message}")]
    PluginManifest { path: String, message: String },
    #[error(transparent)]
    Behavior(#[from] BehaviorError),
}

/// Narrow host adapter. It exposes only pure string/tag helpers and an explicit
/// caller-supplied clock value; the underlying VM is Steel's sandboxed engine.
pub struct SteelWorkspace;

impl SteelWorkspace {
    pub fn load(
        xo_scm: &str,
        modules: &BTreeMap<String, String>,
        deterministic_now: &str,
    ) -> Result<WorkspaceBehavior, SteelConfigError> {
        let mut behavior = evaluate(xo_scm, "workspace-config", deterministic_now)?;
        for (path, source) in modules {
            if valid_module_path(path) {
                let module = evaluate(source, "workspace-module", deterministic_now)?;
                behavior.views.extend(module.views);
                behavior.actions.extend(module.actions);
                behavior.templates.extend(module.templates);
                behavior.capability_grants.extend(module.capability_grants);
            } else if valid_plugin_path(path) {
                merge_plugin(&mut behavior, path, source)?;
            } else {
                return Err(SteelConfigError::InvalidPath(path.clone()));
            }
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

#[derive(Deserialize)]
struct PluginManifest {
    schema: u16,
    actions: Vec<PluginAction>,
}

#[derive(Deserialize)]
struct PluginAction {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    prompt: String,
    #[serde(default = "plugin_entrypoint")]
    entrypoint: String,
    #[serde(default)]
    predicate: Predicate,
    #[serde(default)]
    effects: Vec<ActionEffect>,
    #[serde(default)]
    capabilities: BTreeSet<Capability>,
}

fn plugin_entrypoint() -> String {
    "xo-plugin-run".to_owned()
}

fn merge_plugin(
    behavior: &mut WorkspaceBehavior,
    path: &str,
    source: &str,
) -> Result<(), SteelConfigError> {
    if source.len() > MAX_CONFIG_BYTES {
        return Err(SteelConfigError::TooLarge);
    }
    let mut engine = Engine::new_sandboxed();
    // Manifest discovery compiles the complete plugin without granting host
    // access. These inert signatures make capability calls resolvable; only
    // the action runner installs real, grant-checked implementations.
    engine
        .register_fn("xo-secret", |_name: String| String::new())
        .register_fn(
            "xo-http-post-json",
            |_url: String, _headers: String, _body: String| String::new(),
        );
    let interrupt = InterruptHandler::new(&mut engine, PLUGIN_MANIFEST_TIMEOUT);
    let result = interrupt.run_with_timeout(|| {
        engine.run(source.to_owned())?;
        engine.call_function_by_name_with_args("xo-plugin-manifest", vec![])
    });
    let json = match result {
        Ok(SteelVal::StringV(value)) => value.to_string(),
        Ok(_) => {
            return Err(SteelConfigError::PluginManifest {
                path: path.to_owned(),
                message: "xo-plugin-manifest must return a JSON string".into(),
            });
        }
        Err(error) => {
            return Err(SteelConfigError::PluginManifest {
                path: path.to_owned(),
                message: error.to_string(),
            });
        }
    };
    let manifest: PluginManifest =
        serde_json::from_str(&json).map_err(|error| SteelConfigError::PluginManifest {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    if manifest.schema != 1 {
        return Err(SteelConfigError::PluginManifest {
            path: path.to_owned(),
            message: format!("unsupported schema {}", manifest.schema),
        });
    }
    for action in manifest.actions {
        let plugin = action.effects.is_empty().then(|| ActionPlugin::Steel {
            path: path.to_owned(),
            entrypoint: action.entrypoint,
            prompt: action.prompt,
            capabilities: action.capabilities.clone(),
        });
        behavior
            .capability_grants
            .insert(action.id.clone(), action.capabilities);
        behavior.actions.push(ActionDescriptor {
            id: action.id,
            description: action.description,
            predicate: action.predicate,
            effects: action.effects,
            plugin,
        });
    }
    Ok(())
}

/// Evaluate the native `~/.config/xo/config.scm` schema.
///
/// Only the named field forms are admitted. The parsed values are rebuilt
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
        .register_fn("peer-id", optional_config_value)
        .register_fn("workspace", optional_config_value)
        .register_fn("projection", |value: String| value)
        .register_fn("pwa-url", |value: String| value)
        .register_fn("leader-key", |value: String| value)
        .register_fn(
            "xo-config",
            |schema: String,
             state_dir: String,
             peer_id: String,
             workspace: String,
             projection: String,
             pwa_url: String,
             leader_key: String| {
                serde_json::json!({
                    "schema": schema.parse::<u16>().unwrap_or_default(),
                    "state_dir": state_dir,
                    "peer_id": (!peer_id.is_empty()).then_some(peer_id),
                    "workspace": (!workspace.is_empty()).then_some(workspace),
                    "projection": projection,
                    "pwa_url": pwa_url,
                    "leader_key": leader_key,
                })
                .to_string()
            },
        )
        .register_fn("xo-has-tag", |tags: String, tag: String| {
            tags.split(',').map(str::trim).any(|value| value == tag)
        })
        .register_fn("xo-add-tag", |tags: String, tag: String| {
            update_tags(&tags, &tag, true)
        })
        .register_fn("xo-remove-tag", |tags: String, tag: String| {
            update_tags(&tags, &tag, false)
        });
    let now = now.to_owned();
    engine.register_fn("xo-now", move || now.clone());
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
        schema: optional_u16(&fields, "schema")?
            .ok_or_else(|| native_error("missing field schema"))?,
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
        &["id", "description", "predicate", "effects", "plugin"],
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
        plugin: fields
            .get("plugin")
            .map(|forms| match *forms {
                [form] => parse_plugin(form),
                _ => Err(native_error("plugin expects exactly one value")),
            })
            .transpose()?,
    })
}

fn parse_plugin(form: &NativeForm) -> Result<ActionPlugin, SteelConfigError> {
    let (name, args) = native_call(form)?;
    match (name, args) {
        ("capture-url", []) => Ok(ActionPlugin::CaptureUrl),
        (
            "steel",
            [
                NativeForm::String(path),
                NativeForm::String(entrypoint),
                NativeForm::String(prompt),
                NativeForm::List(capabilities),
            ],
        ) => Ok(ActionPlugin::Steel {
            path: path.clone(),
            entrypoint: entrypoint.clone(),
            prompt: prompt.clone(),
            capabilities: capabilities
                .iter()
                .map(parse_capability)
                .collect::<Result<_, _>>()?,
        }),
        _ => Err(native_error(format!("invalid action plugin {name}"))),
    }
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
        ("set-field", [field, value]) if matches!(native_call(value), Ok(("now", []))) => {
            Ok(ActionEffect::SetFieldNow {
                field: native_string(field, "set-field field")?,
            })
        }
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
        "create-note" => Ok(Capability::CreateNote),
        "mutate-note" => Ok(Capability::MutateNote),
        "network" => Ok(Capability::Network),
        "read-secret" => Ok(Capability::ReadSecret),
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
    peer_id: Option<String>,
    workspace: Option<String>,
    projection: String,
    pwa_url: String,
    leader_key: String,
}

impl NativeXoFields {
    fn canonical(&self) -> String {
        let string = |value: &str| {
            serde_json::to_string(value).expect("native config strings are serializable")
        };
        let optional = |value: Option<&str>| value.map_or_else(|| "#f".to_owned(), string);
        format!(
            "(xo-config (schema {}) (state-dir {}) (peer-id {}) (workspace {}) (projection {}) (pwa-url {}) (leader-key {}))",
            self.schema,
            string(&self.state_dir),
            optional(self.peer_id.as_deref()),
            optional(self.workspace.as_deref()),
            string(&self.projection),
            string(&self.pwa_url),
            string(&self.leader_key),
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
        let mut peer_id = None;
        let mut workspace = None;
        let mut projection = None;
        let mut pwa_url = None;
        let mut leader_key = None;
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
                "peer-id" => set_once(
                    &mut peer_id,
                    self.optional_string()?,
                    "peer-id",
                    self.position,
                )?,
                "workspace" => set_once(
                    &mut workspace,
                    self.optional_string()?,
                    "workspace",
                    self.position,
                )?,
                "projection" => {
                    set_once(&mut projection, self.string()?, "projection", self.position)?;
                }
                "pwa-url" => {
                    set_once(&mut pwa_url, self.string()?, "pwa-url", self.position)?;
                }
                "leader-key" => {
                    set_once(&mut leader_key, self.string()?, "leader-key", self.position)?;
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
            peer_id: peer_id.unwrap_or(None),
            workspace: required(workspace, "workspace", self.position)?,
            projection: required(projection, "projection", self.position)?,
            pwa_url: required(pwa_url, "pwa-url", self.position)?,
            leader_key: required(leader_key, "leader-key", self.position)?,
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
    path == "xo.scm" || valid_module_path(path) || valid_plugin_path(path)
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

fn valid_plugin_path(path: &str) -> bool {
    path.starts_with("plugins/")
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
    output.push(')');
    if let Some(plugin) = &action.plugin {
        write!(output, "\n{field}(plugin {})", encode_plugin(plugin))
            .expect("writing to a String cannot fail");
    }
    output.push(')');
    output
}

fn encode_plugin(plugin: &ActionPlugin) -> String {
    match plugin {
        ActionPlugin::CaptureUrl => "(capture-url)".into(),
        ActionPlugin::Steel {
            path,
            entrypoint,
            prompt,
            capabilities,
        } => format!(
            "(steel {} {} {} ({}))",
            steel_string(path),
            steel_string(entrypoint),
            steel_string(prompt),
            capabilities
                .iter()
                .map(|capability| encode_capability(*capability))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
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
        ActionEffect::SetFieldNow { field } => {
            format!("(set-field {} (now))", steel_string(field))
        }
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
        Capability::CreateNote => "create-note",
        Capability::MutateNote => "mutate-note",
        Capability::Network => "network",
        Capability::ReadSecret => "read-secret",
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
    fn example_workspace_configuration_is_current_and_executable() {
        let behavior = SteelWorkspace::load(
            include_str!("../../../example-config.scm"),
            &BTreeMap::new(),
            "2026-01-02T03:04:05+00:00",
        )
        .unwrap();
        assert_eq!(behavior.default_view, "notes");
        assert_eq!(
            behavior
                .views
                .iter()
                .map(|view| view.id.as_str())
                .collect::<Vec<_>>(),
            vec!["notes", "books", "webhooks", "secrets"]
        );
        assert_eq!(
            behavior.views[1]
                .subviews
                .iter()
                .map(|subview| subview.id.as_str())
                .collect::<Vec<_>>(),
            vec!["all", "to-read", "reading", "read"]
        );
        assert_eq!(behavior.actions.len(), 3);
        assert!(
            behavior
                .capability_grants
                .values()
                .all(|capabilities| capabilities.contains(&Capability::MutateNote))
        );
        let mut note = crate::Note {
            id: crate::NoteId::new("abcdefg"),
            frontmatter: BTreeMap::from([
                ("type".into(), FrontmatterValue::String("note".into())),
                (
                    "tags".into(),
                    FrontmatterValue::Sequence(vec![FrontmatterValue::String("todo".into())]),
                ),
            ]),
            body: String::new(),
            path: "abc/abcdefg-example.md".into(),
        };
        let todo = behavior
            .query(
                std::slice::from_ref(&note),
                &crate::behavior::Query {
                    view: "notes".into(),
                    subview: Some("todo".into()),
                    ..crate::behavior::Query::default()
                },
            )
            .unwrap();
        assert_eq!(todo.len(), 1);
        let now = "2026-01-02T03:04:05+00:00";
        behavior.apply_action(&mut note, "mark-done", now).unwrap();
        assert!(Predicate::HasTag { tag: "done".into() }.matches(&note));

        let mut tagged_note = note.clone();
        tagged_note.frontmatter.insert(
            "tags".into(),
            FrontmatterValue::Sequence(vec![FrontmatterValue::String("to-read".into())]),
        );
        assert!(behavior.action(Some(&tagged_note), "start-book").is_err());

        let mut book = crate::Note {
            id: crate::NoteId::new("bcdefgh"),
            frontmatter: BTreeMap::from([
                ("type".into(), FrontmatterValue::String("book".into())),
                (
                    "tags".into(),
                    FrontmatterValue::Sequence(vec![FrontmatterValue::String("to-read".into())]),
                ),
            ]),
            body: String::new(),
            path: "bcd/bcdefgh-book.md".into(),
        };
        behavior.apply_action(&mut book, "start-book", now).unwrap();
        assert_eq!(
            book.frontmatter.get("started"),
            Some(&FrontmatterValue::String(now.into()))
        );
        assert!(behavior.action(Some(&book), "start-book").is_err());
        assert!(behavior.action(Some(&book), "finish-book").is_ok());
        behavior
            .apply_action(&mut book, "finish-book", now)
            .unwrap();
        assert_eq!(
            book.frontmatter.get("finished"),
            Some(&FrontmatterValue::String(now.into()))
        );
        assert!(behavior.action(Some(&book), "finish-book").is_err());
    }

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
                plugin: None,
            }],
            ..WorkspaceBehavior::default()
        };
        let loaded = SteelWorkspace::load(
            &encode_config(&base, false),
            &BTreeMap::from([(
                "modules/actions/main.scm".into(),
                encode_config(&module, true),
            )]),
            "2026-01-02T03:04:05+00:00",
        )
        .unwrap();
        assert_eq!(loaded.views[0].id, "notes");
        assert_eq!(loaded.actions[0].id, "done");
    }

    #[test]
    fn executable_plugin_manifest_adds_only_hardcover_search() {
        let behavior = SteelWorkspace::load(
            &encode_config(&WorkspaceBehavior::default(), false),
            &BTreeMap::from([(
                "plugins/hardcover.scm".into(),
                include_str!("../../../plugins/hardcover.scm").into(),
            )]),
            "fixed",
        )
        .unwrap();
        let search = behavior
            .actions
            .iter()
            .find(|action| action.id == "hardcover-search")
            .unwrap();
        assert!(matches!(
            search.plugin,
            Some(ActionPlugin::Steel { ref path, .. }) if path == "plugins/hardcover.scm"
        ));
        assert_eq!(behavior.actions.len(), 1);
        assert!(
            behavior
                .actions
                .iter()
                .all(|action| { action.id != "start-book" && action.id != "finish-book" })
        );
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
                    ActionEffect::SetFieldNow {
                        field: "finished".into(),
                    },
                    ActionEffect::AppendBody {
                        text: "\nFinished.\n".into(),
                    },
                ],
                plugin: Some(ActionPlugin::CaptureUrl),
            }],
            templates: vec![TemplateDescriptor {
                id: "daily".into(),
                path: "daily/{{date}}.md".into(),
                content: "---\ntitle: {{date}}\n---\n".into(),
            }],
            capability_grants: BTreeMap::from([(
                "finish".into(),
                BTreeSet::from([
                    Capability::CreateNote,
                    Capability::MutateNote,
                    Capability::Network,
                ]),
            )]),
            query_limit: 42,
        };

        let source = encode_config(&behavior, false);
        assert!(source.starts_with("(workspace-config\n  (schema 1)"));
        assert!(source.contains("(field-equals \"type\" \"note\")"));
        assert!(!source.starts_with("(workspace-config \""));
        let loaded = SteelWorkspace::load(&source, &BTreeMap::new(), "fixed").unwrap();
        assert_eq!(loaded, behavior);
    }

    #[test]
    fn native_workspace_config_rejects_incomplete_and_obsolete_forms() {
        for rejected in [
            "(workspace-config (views))",
            "(workspace-config (schema 0) (views))",
            "(workspace-config \"{\\\"query_limit\\\":42}\")",
        ] {
            assert!(SteelWorkspace::load(rejected, &BTreeMap::new(), "fixed").is_err());
        }
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
        let value = engine.run("(xo-now)".to_owned()).unwrap();
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
            (projection ".")
            (pwa-url "https://xo.exokephalos.dev/"))"#;
        assert!(evaluate_xo_config(native_attack).is_err());
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
