//! Strict migration of the documented legacy Fennel configuration subset.

use std::collections::BTreeSet;

use thiserror::Error;

use crate::behavior::{
    ActionDescriptor, ActionEffect, Capability, Predicate, SubviewDescriptor, ViewDescriptor,
    WorkspaceBehavior,
};
use crate::domain::FrontmatterValue;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MigrationError {
    #[error("legacy Fennel syntax error at byte {offset}: {message}")]
    Syntax { offset: usize, message: String },
    #[error("unsupported legacy construct at {location}: {construct}")]
    Unsupported { location: String, construct: String },
}

#[derive(Clone, Debug)]
enum Form {
    String(String),
    Symbol(String),
    List(Vec<Form>),
    Vector(Vec<Form>),
    Map(Vec<(Form, Form)>),
}

pub fn migrate_fennel(source: &str) -> Result<WorkspaceBehavior, MigrationError> {
    let mut parser = Parser::new(source);
    let root = parser.form()?;
    parser.space();
    if parser.position != source.len() {
        return parser.syntax("unexpected trailing input");
    }
    let root = map(&root, "root")?;
    let default_view = keyword(get(root, "default-view", "root")?, "root.default-view")?;
    let mut behavior = WorkspaceBehavior {
        default_view,
        ..WorkspaceBehavior::default()
    };
    for (key, value) in map(get(root, "views", "root")?, "root.views")? {
        behavior
            .views
            .push(migrate_view(&keyword(key, "root.views key")?, value)?);
    }
    for (key, value) in map(get(root, "actions", "root")?, "root.actions")? {
        let id = keyword(key, "root.actions key")?;
        behavior.actions.push(migrate_action(&id, value)?);
        behavior
            .capability_grants
            .insert(id, BTreeSet::from([Capability::MutateNote]));
    }
    behavior
        .validate()
        .map_err(|error| MigrationError::Unsupported {
            location: "root".into(),
            construct: error.to_string(),
        })?;
    Ok(behavior)
}

#[must_use]
pub fn diagnose_legacy_module(path: &str) -> Option<MigrationError> {
    if std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("lua") || value.eq_ignore_ascii_case("fnl"))
    {
        Some(MigrationError::Unsupported { location: path.to_owned(), construct: "legacy modules are not part of the documented portable subset; rewrite this module as a declarative modules/**/*.scm descriptor".into() })
    } else {
        None
    }
}

fn migrate_view(id: &str, form: &Form) -> Result<ViewDescriptor, MigrationError> {
    let location = format!("views.{id}");
    let values = map(form, &location)?;
    let name = string(get(values, "name", &location)?, &format!("{location}.name"))?;
    let subviews = optional(values, "subviews").map_or(Ok(Vec::new()), |items| {
        vector(items, &format!("{location}.subviews"))?
            .iter()
            .enumerate()
            .map(|(index, item)| migrate_subview(item, &format!("{location}.subviews[{index}]")))
            .collect()
    })?;
    Ok(ViewDescriptor {
        id: id.to_owned(),
        name,
        key: optional(values, "key")
            .map(|value| string(value, &format!("{location}.key")))
            .transpose()?,
        show_tags: optional(values, "show-tags")
            .map(|value| boolean(value, &format!("{location}.show-tags")))
            .transpose()?
            .unwrap_or(false),
        title_field: optional(values, "title-field")
            .map(|value| string(value, &format!("{location}.title-field")))
            .transpose()?
            .unwrap_or_else(|| "title".into()),
        subtitle_field: optional(values, "subtitle-field")
            .map(|value| string(value, &format!("{location}.subtitle-field")))
            .transpose()?,
        sort_field: optional(values, "sort-field")
            .map(|value| string(value, &format!("{location}.sort-field")))
            .transpose()?,
        descending: optional(values, "sort-order")
            .map(|value| string(value, &format!("{location}.sort-order")))
            .transpose()?
            .is_some_and(|value| value == "desc"),
        preview: optional(values, "stats-template")
            .map(|value| string(value, &format!("{location}.stats-template")))
            .transpose()?,
        predicate: predicate(get(values, "when", &location)?, &format!("{location}.when"))?,
        subviews,
    })
}

fn migrate_subview(form: &Form, location: &str) -> Result<SubviewDescriptor, MigrationError> {
    let values = map(form, location)?;
    let name = string(get(values, "name", location)?, &format!("{location}.name"))?;
    Ok(SubviewDescriptor {
        id: identifier(&name),
        name,
        predicate: predicate(get(values, "when", location)?, &format!("{location}.when"))?,
    })
}

fn migrate_action(id: &str, form: &Form) -> Result<ActionDescriptor, MigrationError> {
    let location = format!("actions.{id}");
    let values = map(form, &location)?;
    Ok(ActionDescriptor {
        id: id.to_owned(),
        description: string(
            get(values, "description", &location)?,
            &format!("{location}.description"),
        )?,
        predicate: predicate(get(values, "when", &location)?, &format!("{location}.when"))?,
        effects: effects(get(values, "run", &location)?, &format!("{location}.run"))?,
    })
}

fn predicate(form: &Form, location: &str) -> Result<Predicate, MigrationError> {
    let body = lambda_body(form, location)?;
    predicate_expr(body, location)
}

fn predicate_expr(form: &Form, location: &str) -> Result<Predicate, MigrationError> {
    match form {
        Form::Symbol(value) if value == "true" => Ok(Predicate::Always),
        Form::List(values) if symbol_at(values, 0) == Some("has-tag") && values.len() == 3 => {
            Ok(Predicate::HasTag {
                tag: string(&values[2], location)?,
            })
        }
        Form::List(values) if symbol_at(values, 0) == Some("=") && values.len() == 3 => {
            let field = match &values[1] {
                Form::Symbol(value) if value.starts_with("note.") => value[5..].to_owned(),
                value => return unsupported(location, value),
            };
            Ok(Predicate::FieldEquals {
                field,
                value: string(&values[2], location)?,
            })
        }
        Form::List(values) if symbol_at(values, 0) == Some("not") && values.len() == 2 => {
            Ok(Predicate::Not {
                predicate: Box::new(predicate_expr(&values[1], location)?),
            })
        }
        Form::List(values) if symbol_at(values, 0) == Some("and") => Ok(Predicate::All {
            predicates: values[1..]
                .iter()
                .map(|value| predicate_expr(value, location))
                .collect::<Result<_, _>>()?,
        }),
        Form::List(values) if symbol_at(values, 0) == Some("or") => Ok(Predicate::Any {
            predicates: values[1..]
                .iter()
                .map(|value| predicate_expr(value, location))
                .collect::<Result<_, _>>()?,
        }),
        value => unsupported(location, value),
    }
}

fn effects(form: &Form, location: &str) -> Result<Vec<ActionEffect>, MigrationError> {
    let body = lambda_body(form, location)?;
    let Form::List(values) = body else {
        return unsupported(location, body);
    };
    if symbol_at(values, 0) != Some("assoc")
        || values.len() != 4
        || symbol(&values[1]) != Some("note")
    {
        return unsupported(location, body);
    }
    let target = keyword(&values[2], location)?;
    if target == "tags" {
        let mut result = Vec::new();
        tag_effects(&values[3], location, &mut result)?;
        Ok(result)
    } else {
        Ok(vec![ActionEffect::SetField {
            field: target,
            value: FrontmatterValue::String(string(&values[3], location)?),
        }])
    }
}

fn tag_effects(
    form: &Form,
    location: &str,
    effects: &mut Vec<ActionEffect>,
) -> Result<(), MigrationError> {
    match form {
        Form::Symbol(value) if value == "note.tags" => Ok(()),
        Form::List(values)
            if values.len() == 3
                && matches!(symbol_at(values, 0), Some("add-tag" | "remove-tag")) =>
        {
            tag_effects(&values[1], location, effects)?;
            let tag = string(&values[2], location)?;
            effects.push(if symbol_at(values, 0) == Some("add-tag") {
                ActionEffect::AddTag { tag }
            } else {
                ActionEffect::RemoveTag { tag }
            });
            Ok(())
        }
        value => unsupported(location, value),
    }
}

fn lambda_body<'a>(form: &'a Form, location: &str) -> Result<&'a Form, MigrationError> {
    match form {
        Form::List(values)
            if symbol_at(values, 0) == Some("fn")
                && values.len() == 3
                && matches!(values[1], Form::Vector(_)) =>
        {
            Ok(&values[2])
        }
        value => unsupported(location, value),
    }
}

fn map<'a>(form: &'a Form, location: &str) -> Result<&'a [(Form, Form)], MigrationError> {
    match form {
        Form::Map(values) => Ok(values),
        value => unsupported(location, value),
    }
}
fn vector<'a>(form: &'a Form, location: &str) -> Result<&'a [Form], MigrationError> {
    match form {
        Form::Vector(values) => Ok(values),
        value => unsupported(location, value),
    }
}
fn get<'a>(
    values: &'a [(Form, Form)],
    key: &str,
    location: &str,
) -> Result<&'a Form, MigrationError> {
    optional(values, key).ok_or_else(|| MigrationError::Unsupported {
        location: location.into(),
        construct: format!("missing required :{key}"),
    })
}
fn optional<'a>(values: &'a [(Form, Form)], key: &str) -> Option<&'a Form> {
    values.iter().find(|(candidate, _)| matches!(candidate, Form::Symbol(value) if value == &format!(":{key}"))).map(|(_, value)| value)
}
fn keyword(form: &Form, location: &str) -> Result<String, MigrationError> {
    match form {
        Form::Symbol(value) if value.starts_with(':') => Ok(value[1..].to_owned()),
        value => unsupported(location, value),
    }
}
fn string(form: &Form, location: &str) -> Result<String, MigrationError> {
    match form {
        Form::String(value) => Ok(value.clone()),
        value => unsupported(location, value),
    }
}
fn boolean(form: &Form, location: &str) -> Result<bool, MigrationError> {
    match form {
        Form::Symbol(value) if value == "true" => Ok(true),
        Form::Symbol(value) if value == "false" => Ok(false),
        value => unsupported(location, value),
    }
}
fn symbol(form: &Form) -> Option<&str> {
    if let Form::Symbol(value) = form {
        Some(value)
    } else {
        None
    }
}
fn symbol_at(values: &[Form], index: usize) -> Option<&str> {
    values.get(index).and_then(symbol)
}
fn unsupported<T>(location: &str, form: &Form) -> Result<T, MigrationError> {
    Err(MigrationError::Unsupported {
        location: location.into(),
        construct: format!("{form:?}"),
    })
}
fn identifier(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() {
                value
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
}
impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }
    fn form(&mut self) -> Result<Form, MigrationError> {
        self.space();
        let Some(ch) = self.peek() else {
            return self.syntax("expected form");
        };
        match ch {
            '(' => self.sequence(')', Form::List),
            '[' => self.sequence(']', Form::Vector),
            '{' => {
                let values = self.raw_sequence('}')?;
                if values.len() % 2 != 0 {
                    return self.syntax("map has an odd number of forms");
                }
                Ok(Form::Map(
                    values
                        .chunks_exact(2)
                        .map(|pair| (pair[0].clone(), pair[1].clone()))
                        .collect(),
                ))
            }
            '"' => self.string(),
            _ => self.symbol(),
        }
    }
    fn sequence(
        &mut self,
        end: char,
        build: impl FnOnce(Vec<Form>) -> Form,
    ) -> Result<Form, MigrationError> {
        Ok(build(self.raw_sequence(end)?))
    }
    fn raw_sequence(&mut self, end: char) -> Result<Vec<Form>, MigrationError> {
        self.bump();
        let mut values = Vec::new();
        loop {
            self.space();
            match self.peek() {
                Some(value) if value == end => {
                    self.bump();
                    return Ok(values);
                }
                None => return self.syntax("unterminated collection"),
                _ => values.push(self.form()?),
            }
        }
    }
    fn string(&mut self) -> Result<Form, MigrationError> {
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(Form::String(value)),
                Some('\\') => match self.bump() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    _ => return self.syntax("unsupported string escape"),
                },
                Some(ch) => value.push(ch),
                None => return self.syntax("unterminated string"),
            }
        }
    }
    fn symbol(&mut self) -> Result<Form, MigrationError> {
        let start = self.position;
        while self
            .peek()
            .is_some_and(|ch| !ch.is_whitespace() && !"()[]{}\";".contains(ch))
        {
            self.bump();
        }
        if start == self.position {
            return self.syntax("unexpected character");
        }
        Ok(Form::Symbol(self.source[start..self.position].to_owned()))
    }
    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() == Some(';') {
                while self.peek().is_some_and(|ch| ch != '\n') {
                    self.bump();
                }
            } else {
                break;
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
    fn syntax<T>(&self, message: &str) -> Result<T, MigrationError> {
        Err(MigrationError::Syntax {
            offset: self.position,
            message: message.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrates_example_views_and_actions_without_executing_fennel() {
        let behavior =
            migrate_fennel(include_str!("../../../oldcodebase/example-repo/exo.fnl")).unwrap();
        assert_eq!(behavior.default_view, "notes");
        assert_eq!(behavior.views.len(), 5);
        assert_eq!(behavior.actions.len(), 3);
        assert_eq!(
            behavior
                .views
                .iter()
                .find(|view| view.id == "books")
                .unwrap()
                .subviews
                .len(),
            4
        );
        assert_eq!(
            behavior
                .actions
                .iter()
                .find(|action| action.id == "finish-book")
                .unwrap()
                .effects
                .len(),
            2
        );
    }
    #[test]
    fn diagnoses_unknown_code() {
        assert!(matches!(
            migrate_fennel(
                "{:default-view :all :views {} :actions {:x {:description \"x\" :when (fn [n] true) :run (fn [n] (os.execute \"id\"))}}}"
            ),
            Err(MigrationError::Unsupported { .. })
        ));
    }
}
