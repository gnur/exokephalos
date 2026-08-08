//! Portable, declarative workspace behavior shared by Steel and native clients.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Note;
use crate::domain::FrontmatterValue;

pub const BEHAVIOR_SCHEMA: u16 = 1;
pub const DEFAULT_QUERY_LIMIT: usize = 500;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum BehaviorError {
    #[error("unsupported behavior schema {0}")]
    UnsupportedSchema(u16),
    #[error("duplicate {kind} identifier: {id}")]
    Duplicate { kind: &'static str, id: String },
    #[error("unknown view: {0}")]
    UnknownView(String),
    #[error("unknown subview: {0}")]
    UnknownSubview(String),
    #[error("unknown action: {0}")]
    UnknownAction(String),
    #[error("action is unavailable for this note: {0}")]
    ActionUnavailable(String),
    #[error("action {action} lacks capability grant {capability:?}")]
    CapabilityDenied {
        action: String,
        capability: Capability,
    },
    #[error("query limit must be between 1 and {DEFAULT_QUERY_LIMIT}")]
    InvalidQueryLimit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceBehavior {
    pub schema: u16,
    #[serde(default = "default_view")]
    pub default_view: String,
    #[serde(default)]
    pub views: Vec<ViewDescriptor>,
    #[serde(default)]
    pub actions: Vec<ActionDescriptor>,
    #[serde(default)]
    pub templates: Vec<TemplateDescriptor>,
    #[serde(default)]
    pub capability_grants: BTreeMap<String, BTreeSet<Capability>>,
    #[serde(default = "query_limit")]
    pub query_limit: usize,
}

fn default_view() -> String {
    "all".to_owned()
}
const fn query_limit() -> usize {
    DEFAULT_QUERY_LIMIT
}

impl Default for WorkspaceBehavior {
    fn default() -> Self {
        Self {
            schema: BEHAVIOR_SCHEMA,
            default_view: default_view(),
            views: Vec::new(),
            actions: Vec::new(),
            templates: Vec::new(),
            capability_grants: BTreeMap::new(),
            query_limit: DEFAULT_QUERY_LIMIT,
        }
    }
}

/// Built-in views used until a workspace publishes its own Steel configuration.
#[must_use]
pub fn default_views() -> Vec<ViewDescriptor> {
    vec![
        ViewDescriptor {
            id: "notes".into(),
            name: "Notes".into(),
            key: Some("n".into()),
            show_tags: true,
            title_field: "title".into(),
            subtitle_field: None,
            sort_field: Some("created".into()),
            descending: true,
            preview: None,
            predicate: Predicate::FieldEquals {
                field: "type".into(),
                value: "note".into(),
            },
            subviews: vec![],
        },
        ViewDescriptor {
            id: "all".into(),
            name: "All".into(),
            key: Some("0".into()),
            show_tags: true,
            title_field: "title".into(),
            subtitle_field: Some("type".into()),
            sort_field: Some("created".into()),
            descending: true,
            preview: None,
            predicate: Predicate::Always,
            subviews: vec![],
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewDescriptor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub show_tags: bool,
    #[serde(default = "title_field")]
    pub title_field: String,
    #[serde(default)]
    pub subtitle_field: Option<String>,
    #[serde(default)]
    pub sort_field: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub predicate: Predicate,
    #[serde(default)]
    pub subviews: Vec<SubviewDescriptor>,
}

fn title_field() -> String {
    "title".to_owned()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubviewDescriptor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub predicate: Predicate,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Predicate {
    #[default]
    Always,
    FieldEquals {
        field: String,
        value: String,
    },
    HasTag {
        tag: String,
    },
    Not {
        predicate: Box<Self>,
    },
    All {
        predicates: Vec<Self>,
    },
    Any {
        predicates: Vec<Self>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionDescriptor {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub predicate: Predicate,
    #[serde(default)]
    pub effects: Vec<ActionEffect>,
    #[serde(default)]
    pub plugin: Option<ActionPlugin>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionPlugin {
    CaptureUrl,
    Steel {
        path: String,
        entrypoint: String,
        prompt: String,
        capabilities: BTreeSet<Capability>,
    },
}

impl ActionPlugin {
    #[must_use]
    pub fn required_capabilities(&self) -> BTreeSet<Capability> {
        match self {
            Self::CaptureUrl => BTreeSet::from([Capability::CreateNote, Capability::Network]),
            Self::Steel { capabilities, .. } => capabilities.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "kebab-case")]
pub enum ActionEffect {
    AddTag {
        tag: String,
    },
    RemoveTag {
        tag: String,
    },
    SetField {
        field: String,
        value: FrontmatterValue,
    },
    SetFieldNow {
        field: String,
    },
    AppendBody {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    CreateNote,
    MutateNote,
    Network,
    ReadSecret,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemplateDescriptor {
    pub id: String,
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Query {
    pub view: String,
    pub subview: Option<String>,
    pub title: Option<String>,
    pub tags: BTreeSet<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateInputs {
    pub date: String,
    pub date_time: String,
    pub year: String,
    pub month: String,
    pub day: String,
    pub id: String,
    pub slug: String,
    pub values: BTreeMap<String, String>,
}

impl TemplateInputs {
    /// Build every clock-derived template value from an explicit instant.
    pub fn deterministic(
        instant: time::OffsetDateTime,
        id: impl Into<String>,
        slug: impl Into<String>,
        values: BTreeMap<String, String>,
    ) -> Result<Self, time::error::Format> {
        use time::macros::format_description;
        Ok(Self {
            date: instant
                .date()
                .format(format_description!("[year]-[month]-[day]"))?,
            date_time: crate::timestamp::format(instant)?,
            year: instant.year().to_string(),
            month: format!("{:02}", u8::from(instant.month())),
            day: format!("{:02}", instant.day()),
            id: id.into(),
            slug: slug.into(),
            values,
        })
    }
}

impl WorkspaceBehavior {
    pub fn validate(&self) -> Result<(), BehaviorError> {
        if self.schema != BEHAVIOR_SCHEMA {
            return Err(BehaviorError::UnsupportedSchema(self.schema));
        }
        if !(1..=DEFAULT_QUERY_LIMIT).contains(&self.query_limit) {
            return Err(BehaviorError::InvalidQueryLimit);
        }
        unique("view", self.views.iter().map(|value| value.id.as_str()))?;
        unique("action", self.actions.iter().map(|value| value.id.as_str()))?;
        unique(
            "template",
            self.templates.iter().map(|value| value.id.as_str()),
        )?;
        for view in &self.views {
            unique(
                "subview",
                view.subviews.iter().map(|value| value.id.as_str()),
            )?;
        }
        if self.default_view != "all" && !self.views.iter().any(|view| view.id == self.default_view)
        {
            return Err(BehaviorError::UnknownView(self.default_view.clone()));
        }
        Ok(())
    }

    #[must_use]
    pub fn declarative_descriptor(&self) -> Self {
        self.clone()
    }

    pub fn query<'a>(
        &self,
        notes: &'a [Note],
        query: &Query,
    ) -> Result<Vec<&'a Note>, BehaviorError> {
        self.validate()?;
        let view = if query.view.is_empty() || query.view == "all" {
            None
        } else {
            Some(
                self.views
                    .iter()
                    .find(|view| view.id == query.view)
                    .ok_or_else(|| BehaviorError::UnknownView(query.view.clone()))?,
            )
        };
        let subview = match (&view, &query.subview) {
            (_, None) => None,
            (Some(view), Some(id)) => Some(
                view.subviews
                    .iter()
                    .find(|item| item.id == *id)
                    .ok_or_else(|| BehaviorError::UnknownSubview(id.clone()))?,
            ),
            (None, Some(id)) => return Err(BehaviorError::UnknownSubview(id.clone())),
        };
        let title = query.title.as_deref().map(str::to_lowercase);
        let limit = query
            .limit
            .unwrap_or(self.query_limit)
            .min(self.query_limit);
        let mut result = notes
            .iter()
            .filter(|note| {
                view.is_none_or(|value| value.predicate.matches(note))
                    && subview.is_none_or(|value| value.predicate.matches(note))
                    && title
                        .as_ref()
                        .is_none_or(|needle| field(note, "title").to_lowercase().contains(needle))
                    && query.tags.iter().all(|tag| tags(note).contains(tag))
            })
            .collect::<Vec<_>>();
        let sort_field = view
            .and_then(|value| value.sort_field.as_deref())
            .unwrap_or("created");
        result.sort_by(|left, right| {
            field(left, sort_field)
                .to_lowercase()
                .cmp(&field(right, sort_field).to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        if view.is_some_and(|value| value.descending) {
            result.reverse();
        }
        result.truncate(limit);
        Ok(result)
    }

    pub fn action(
        &self,
        note: Option<&Note>,
        id: &str,
    ) -> Result<&ActionDescriptor, BehaviorError> {
        let action = self
            .actions
            .iter()
            .find(|value| value.id == id)
            .ok_or_else(|| BehaviorError::UnknownAction(id.to_owned()))?;
        if !action
            .plugin
            .as_ref()
            .is_some_and(|_| note.is_none() && action.predicate == Predicate::Always)
            && !note.is_some_and(|note| action.predicate.matches(note))
        {
            return Err(BehaviorError::ActionUnavailable(id.to_owned()));
        }
        let grants = self.capability_grants.get(id);
        let mut required = action
            .plugin
            .as_ref()
            .map(ActionPlugin::required_capabilities)
            .unwrap_or_default();
        if !action.effects.is_empty() {
            required.insert(Capability::MutateNote);
        }
        for capability in required {
            if !grants.is_some_and(|grants| grants.contains(&capability)) {
                return Err(BehaviorError::CapabilityDenied {
                    action: id.to_owned(),
                    capability,
                });
            }
        }
        Ok(action)
    }

    pub fn apply_action(&self, note: &mut Note, id: &str, now: &str) -> Result<(), BehaviorError> {
        let action = self.action(Some(note), id)?;
        for effect in &action.effects {
            effect.apply(note, now);
        }
        Ok(())
    }
}

impl Predicate {
    #[must_use]
    pub fn matches(&self, note: &Note) -> bool {
        match self {
            Self::Always => true,
            Self::FieldEquals { field: name, value } => field(note, name) == *value,
            Self::HasTag { tag } => tags(note).contains(tag),
            Self::Not { predicate } => !predicate.matches(note),
            Self::All { predicates } => predicates.iter().all(|value| value.matches(note)),
            Self::Any { predicates } => predicates.iter().any(|value| value.matches(note)),
        }
    }
}

impl ActionEffect {
    fn apply(&self, note: &mut Note, now: &str) {
        match self {
            Self::AddTag { tag } => {
                let mut values = tags(note);
                values.insert(tag.clone());
                set_tags(note, values);
            }
            Self::RemoveTag { tag } => {
                let mut values = tags(note);
                values.remove(tag);
                set_tags(note, values);
            }
            Self::SetField { field, value } => {
                note.frontmatter.insert(field.clone(), value.clone());
            }
            Self::SetFieldNow { field } => {
                note.frontmatter
                    .insert(field.clone(), FrontmatterValue::String(now.to_owned()));
            }
            Self::AppendBody { text } => note.body.push_str(text),
        }
    }
}

#[must_use]
pub fn render_preview(template: &str, note: &Note) -> String {
    let mut values = BTreeMap::from([
        ("ID".to_owned(), note.id.to_string()),
        ("Body".to_owned(), note.body.clone()),
    ]);
    for (key, value) in &note.frontmatter {
        values.insert(key.clone(), display_value(value));
    }
    render_variables(template, &values)
}

#[must_use]
pub fn render_template(template: &str, input: &TemplateInputs) -> String {
    let mut values = input.values.clone();
    values.extend([
        ("Date".to_owned(), input.date.clone()),
        ("DateTime".to_owned(), input.date_time.clone()),
        ("Year".to_owned(), input.year.clone()),
        ("Month".to_owned(), input.month.clone()),
        ("Day".to_owned(), input.day.clone()),
        ("ID".to_owned(), input.id.clone()),
        ("Slug".to_owned(), input.slug.clone()),
    ]);
    render_variables(template, &values)
}

fn render_variables(template: &str, values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{{{key}}}}}"), value)
        })
}

fn unique<'a>(
    kind: &'static str,
    values: impl Iterator<Item = &'a str>,
) -> Result<(), BehaviorError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.is_empty() || !seen.insert(value) {
            return Err(BehaviorError::Duplicate {
                kind,
                id: value.to_owned(),
            });
        }
    }
    Ok(())
}

/// Return the four-digit year represented by a configured sort field.
///
/// Date and timestamp fields conventionally begin with an ISO year. Values
/// without such a prefix are grouped under an undated heading by clients.
#[must_use]
pub fn sort_year(note: &Note, sort_field: &str) -> Option<String> {
    let value = field(note, sort_field);
    let year = value.get(..4)?;
    (year.chars().all(|character| character.is_ascii_digit())).then(|| year.to_owned())
}

fn field(note: &Note, name: &str) -> String {
    if name == "id" {
        return note.id.to_string();
    }
    if name == "path" {
        return note.path.clone();
    }
    note.frontmatter
        .get(name)
        .map(display_value)
        .unwrap_or_default()
}

fn display_value(value: &FrontmatterValue) -> String {
    match value {
        FrontmatterValue::Null => String::new(),
        FrontmatterValue::Bool(value) => value.to_string(),
        FrontmatterValue::Integer(value) => value.to_string(),
        FrontmatterValue::Float(value) => value.to_string(),
        FrontmatterValue::String(value) => value.clone(),
        FrontmatterValue::Sequence(values) => values
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        FrontmatterValue::Mapping(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn tags(note: &Note) -> BTreeSet<String> {
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
        _ => BTreeSet::new(),
    }
}

fn set_tags(note: &mut Note, tags: BTreeSet<String>) {
    note.frontmatter.insert(
        "tags".to_owned(),
        FrontmatterValue::Sequence(tags.into_iter().map(FrontmatterValue::String).collect()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoteId;

    fn note(title: &str, tags: &[&str]) -> Note {
        Note {
            id: NoteId::new(title.to_lowercase()),
            frontmatter: BTreeMap::from([
                (
                    "title".to_owned(),
                    FrontmatterValue::String(title.to_owned()),
                ),
                (
                    "tags".to_owned(),
                    FrontmatterValue::Sequence(
                        tags.iter()
                            .map(|tag| FrontmatterValue::String((*tag).to_owned()))
                            .collect(),
                    ),
                ),
            ]),
            body: "body".to_owned(),
            path: format!("{title}.md"),
        }
    }

    #[test]
    fn default_sort_uses_created_and_extracts_year_headers() {
        let mut older = note("Z title", &[]);
        older.frontmatter.insert(
            "created".into(),
            FrontmatterValue::String("2024-03-01T10:00:00Z".into()),
        );
        let mut newer = note("A title", &[]);
        newer.frontmatter.insert(
            "created".into(),
            FrontmatterValue::String("2025-04-01T10:00:00Z".into()),
        );
        let behavior = WorkspaceBehavior::default();
        let notes = [newer, older];
        let found = behavior.query(&notes, &Query::default()).unwrap();
        assert_eq!(found[0].id.as_str(), "z title");
        assert_eq!(sort_year(found[0], "created").as_deref(), Some("2024"));
        assert_eq!(sort_year(found[1], "title"), None);
    }

    #[test]
    fn queries_are_bounded_and_actions_require_grants() {
        let mut behavior = WorkspaceBehavior {
            views: vec![ViewDescriptor {
                id: "books".into(),
                name: "Books".into(),
                key: None,
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: None,
                sort_field: None,
                descending: false,
                preview: None,
                predicate: Predicate::HasTag { tag: "book".into() },
                subviews: vec![],
            }],
            actions: vec![ActionDescriptor {
                id: "finish".into(),
                description: String::new(),
                predicate: Predicate::HasTag {
                    tag: "reading".into(),
                },
                effects: vec![
                    ActionEffect::RemoveTag {
                        tag: "reading".into(),
                    },
                    ActionEffect::AddTag {
                        tag: "finished".into(),
                    },
                ],
                plugin: None,
            }],
            ..WorkspaceBehavior::default()
        };
        let notes = vec![
            note("B", &["book"]),
            note("A", &["book", "reading"]),
            note("N", &["note"]),
        ];
        let found = behavior
            .query(
                &notes,
                &Query {
                    view: "books".into(),
                    limit: Some(1),
                    ..Query::default()
                },
            )
            .unwrap();
        assert_eq!(found[0].id.as_str(), "a");
        let mut target = notes[1].clone();
        assert!(matches!(
            behavior.apply_action(&mut target, "finish", "fixed"),
            Err(BehaviorError::CapabilityDenied { .. })
        ));
        behavior
            .capability_grants
            .insert("finish".into(), BTreeSet::from([Capability::MutateNote]));
        behavior
            .apply_action(&mut target, "finish", "fixed")
            .unwrap();
        assert!(
            Predicate::HasTag {
                tag: "finished".into()
            }
            .matches(&target)
        );
    }

    #[test]
    fn plugin_actions_require_every_host_capability() {
        let mut behavior = WorkspaceBehavior {
            actions: vec![ActionDescriptor {
                id: "capture-url".into(),
                description: "Capture URL".into(),
                predicate: Predicate::Always,
                effects: vec![],
                plugin: Some(ActionPlugin::CaptureUrl),
            }],
            ..WorkspaceBehavior::default()
        };
        assert_eq!(
            behavior.action(None, "capture-url"),
            Err(BehaviorError::CapabilityDenied {
                action: "capture-url".into(),
                capability: Capability::CreateNote,
            })
        );
        behavior.capability_grants.insert(
            "capture-url".into(),
            BTreeSet::from([Capability::CreateNote]),
        );
        assert_eq!(
            behavior.action(None, "capture-url"),
            Err(BehaviorError::CapabilityDenied {
                action: "capture-url".into(),
                capability: Capability::Network,
            })
        );
        behavior
            .capability_grants
            .get_mut("capture-url")
            .unwrap()
            .insert(Capability::Network);
        assert_eq!(
            behavior.action(None, "capture-url").unwrap().plugin,
            Some(ActionPlugin::CaptureUrl)
        );
    }
}
