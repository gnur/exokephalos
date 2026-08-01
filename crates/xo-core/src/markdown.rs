use thiserror::Error;

use crate::NoteId;
use crate::domain::{Frontmatter, FrontmatterValue};

#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownDocument {
    pub frontmatter: Option<Frontmatter>,
    pub body: String,
}

#[derive(Debug, Error)]
pub enum MarkdownError {
    #[error("invalid YAML frontmatter: {0}")]
    InvalidYaml(#[from] serde_yaml::Error),
    #[error("frontmatter keys must be strings")]
    NonStringKey,
    #[error("unsupported YAML value")]
    UnsupportedValue,
}

pub fn parse(content: &str) -> Result<MarkdownDocument, MarkdownError> {
    let Some(rest) = content.strip_prefix("---") else {
        return Ok(MarkdownDocument {
            frontmatter: None,
            body: content.to_owned(),
        });
    };
    let Some(end) = rest.find("---") else {
        return Ok(MarkdownDocument {
            frontmatter: None,
            body: content.to_owned(),
        });
    };
    let raw = &rest[..end];
    let body = rest[end + 3..]
        .strip_prefix('\n')
        .unwrap_or(&rest[end + 3..]);
    let yaml: serde_yaml::Value = serde_yaml::from_str(raw)?;
    let frontmatter = match yaml {
        serde_yaml::Value::Null => Frontmatter::new(),
        serde_yaml::Value::Mapping(mapping) => convert_mapping(mapping)?,
        _ => return Err(MarkdownError::UnsupportedValue),
    };
    Ok(MarkdownDocument {
        frontmatter: Some(frontmatter),
        body: body.to_owned(),
    })
}

pub fn render(frontmatter: &Frontmatter, body: &str) -> Result<String, MarkdownError> {
    let yaml = serde_yaml::to_string(frontmatter)?;
    Ok(format!("---\n{yaml}---\n{body}"))
}

#[must_use]
pub fn tags(frontmatter: &Frontmatter) -> Vec<&str> {
    match frontmatter.get("tags") {
        Some(FrontmatterValue::Sequence(values)) => values
            .iter()
            .filter_map(|value| match value {
                FrontmatterValue::String(tag) => Some(tag.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[must_use]
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

#[must_use]
pub fn canonical_note_path(id: &NoteId, frontmatter: &Frontmatter) -> String {
    let title = match frontmatter.get("title") {
        Some(FrontmatterValue::String(title)) => title.as_str(),
        _ => "untitled",
    };
    let slug = slugify(title);
    let slug = if slug.is_empty() { "untitled" } else { &slug };
    let prefix = id.as_str().chars().take(3).collect::<String>();
    format!("{prefix}/{id}-{slug}.md")
}

#[must_use]
pub fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in value.to_lowercase().chars() {
        if character.is_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
        if slug.chars().count() >= 50 {
            break;
        }
    }
    slug.trim_end_matches('-').to_owned()
}

fn convert_mapping(mapping: serde_yaml::Mapping) -> Result<Frontmatter, MarkdownError> {
    mapping
        .into_iter()
        .map(|(key, value)| {
            let serde_yaml::Value::String(key) = key else {
                return Err(MarkdownError::NonStringKey);
            };
            Ok((key, convert_value(value)?))
        })
        .collect()
}

fn convert_value(value: serde_yaml::Value) -> Result<FrontmatterValue, MarkdownError> {
    Ok(match value {
        serde_yaml::Value::Null => FrontmatterValue::Null,
        serde_yaml::Value::Bool(value) => FrontmatterValue::Bool(value),
        serde_yaml::Value::Number(value) if value.is_i64() => {
            FrontmatterValue::Integer(value.as_i64().unwrap())
        }
        serde_yaml::Value::Number(value) if value.is_u64() => FrontmatterValue::Integer(
            i64::try_from(value.as_u64().unwrap()).map_err(|_| MarkdownError::UnsupportedValue)?,
        ),
        serde_yaml::Value::Number(value) => {
            FrontmatterValue::Float(value.as_f64().ok_or(MarkdownError::UnsupportedValue)?)
        }
        serde_yaml::Value::String(value) => FrontmatterValue::String(value),
        serde_yaml::Value::Sequence(values) => FrontmatterValue::Sequence(
            values
                .into_iter()
                .map(convert_value)
                .collect::<Result<_, _>>()?,
        ),
        serde_yaml::Value::Mapping(value) => FrontmatterValue::Mapping(convert_mapping(value)?),
        serde_yaml::Value::Tagged(value) => convert_value(value.value)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_recursive_frontmatter() {
        let input = "---\ntitle: Hello\ntags: [rust, notes]\nnested:\n  answer: 42\n---\n# Body\n";
        let document = parse(input).unwrap();
        let frontmatter = document.frontmatter.unwrap();
        assert_eq!(tags(&frontmatter), vec!["rust", "notes"]);
        let rendered = render(&frontmatter, &document.body).unwrap();
        assert_eq!(parse(&rendered).unwrap().frontmatter, Some(frontmatter));
        assert!(rendered.ends_with("# Body\n"));
    }

    #[test]
    fn content_without_frontmatter_is_untouched() {
        let input = "# Just Markdown\n";
        assert_eq!(
            parse(input).unwrap(),
            MarkdownDocument {
                frontmatter: None,
                body: input.to_owned()
            }
        );
    }

    #[test]
    fn slug_is_normalized_and_bounded() {
        assert_eq!(slugify("  Héllo, Rust World!  "), "héllo-rust-world");
    }
}
