use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::domain::{FrontmatterValue, Note};
use crate::{NoteId, id, markdown};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    pub notes: Vec<Note>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("projection I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid projection path: {0}")]
    InvalidPath(String),
    #[error("Markdown serialization failed: {0}")]
    Markdown(#[from] markdown::MarkdownError),
    #[error("atomic projection persist failed: {0}")]
    Persist(#[from] tempfile::PersistError),
}

pub fn scan(root: impl AsRef<Path>) -> Result<ScanReport, ProjectionError> {
    let root = root.as_ref();
    let mut paths = Vec::new();
    collect_markdown(root, root, &mut paths)?;
    paths.sort();

    let mut report = ScanReport::default();
    let mut ids = BTreeMap::<NoteId, String>::new();
    for path in paths {
        let relative = relative_string(root, &path)?;
        let content = std::fs::read_to_string(&path)?;
        let document = match markdown::parse(&content) {
            Ok(document) => document,
            Err(error) => {
                report.diagnostics.push(Diagnostic {
                    path: relative,
                    code: "malformed-markdown".to_owned(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        let Some(frontmatter) = document.frontmatter else {
            report.diagnostics.push(Diagnostic {
                path: relative,
                code: "missing-frontmatter".to_owned(),
                message: "Markdown document has no YAML frontmatter".to_owned(),
            });
            continue;
        };
        let Some(FrontmatterValue::String(note_id)) = frontmatter.get("id") else {
            report.diagnostics.push(Diagnostic {
                path: relative,
                code: "missing-id".to_owned(),
                message: "frontmatter has no string id".to_owned(),
            });
            continue;
        };
        if !id::is_valid(note_id) {
            report.diagnostics.push(Diagnostic {
                path: relative,
                code: "invalid-id".to_owned(),
                message: format!("invalid note ID: {note_id}"),
            });
            continue;
        }
        let id = NoteId::new(note_id);
        if let Some(first_path) = ids.insert(id.clone(), relative.clone()) {
            report.diagnostics.push(Diagnostic {
                path: relative,
                code: "duplicate-id".to_owned(),
                message: format!("note ID {id} is already used by {first_path}"),
            });
            continue;
        }
        report.notes.push(Note {
            id,
            frontmatter,
            body: document.body,
            path: relative,
        });
    }
    Ok(report)
}

pub fn materialize(root: impl AsRef<Path>, note: &Note) -> Result<PathBuf, ProjectionError> {
    let root = root.as_ref();
    let destination = safe_join(root, &note.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| ProjectionError::InvalidPath(note.path.clone()))?;
    std::fs::create_dir_all(parent)?;
    let content = markdown::render(&note.frontmatter, &note.body)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(&destination)?;
    Ok(destination)
}

fn collect_markdown(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if relative.components().any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
            }) {
                continue;
            }
            collect_markdown(root, &path, paths)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            paths.push(path);
        }
    }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, ProjectionError> {
    let relative_path = Path::new(relative);
    let invalid = relative.is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || relative_path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".exo");
    if invalid {
        Err(ProjectionError::InvalidPath(relative.to_owned()))
    } else {
        Ok(root.join(relative_path))
    }
}

fn relative_string(root: &Path, path: &Path) -> Result<String, ProjectionError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ProjectionError::InvalidPath(path.display().to_string()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Frontmatter, FrontmatterValue};

    fn note(id: &str, path: &str) -> Note {
        Note {
            id: NoteId::new(id),
            frontmatter: Frontmatter::from([
                ("id".to_owned(), FrontmatterValue::String(id.to_owned())),
                (
                    "title".to_owned(),
                    FrontmatterValue::String("Title".to_owned()),
                ),
                (
                    "type".to_owned(),
                    FrontmatterValue::String("note".to_owned()),
                ),
            ]),
            body: "body\n".to_owned(),
            path: path.to_owned(),
        }
    }

    #[test]
    fn materialize_and_scan_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let expected = note("note002", "notes/one.md");
        materialize(directory.path(), &expected).unwrap();
        let report = scan(directory.path()).unwrap();
        assert_eq!(report.notes, vec![expected]);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn scan_diagnoses_duplicates_and_ignores_hidden_state() {
        let directory = tempfile::tempdir().unwrap();
        materialize(directory.path(), &note("note002", "notes/one.md")).unwrap();
        materialize(directory.path(), &note("note002", "notes/two.md")).unwrap();
        materialize(directory.path(), &note("hide002", ".exo/hidden.md")).unwrap_err();
        let report = scan(directory.path()).unwrap();
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.diagnostics[0].code, "duplicate-id");
    }

    #[test]
    fn materialize_rejects_path_traversal() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            materialize(directory.path(), &note("note002", "../escape.md")),
            Err(ProjectionError::InvalidPath(_))
        ));
    }
}
