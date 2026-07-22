use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime};

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
    #[error("expected-write state is invalid: {0}")]
    ExpectedWriteState(#[from] serde_json::Error),
    #[error("expected-write state lock was poisoned")]
    Poisoned,
}

/// Hashes of projection writes that a filesystem watcher must consume, not import.
#[derive(Debug, Default)]
pub struct ExpectedWrites {
    hashes: Mutex<BTreeMap<String, String>>,
    state_path: Option<PathBuf>,
}

impl ExpectedWrites {
    /// Load durable suppression state, normally from `.exo/expected-writes.json`.
    pub fn open(state_path: impl AsRef<Path>) -> Result<Self, ProjectionError> {
        let state_path = state_path.as_ref().to_path_buf();
        let hashes = match std::fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            hashes: Mutex::new(hashes),
            state_path: Some(state_path),
        })
    }

    /// Consume a watcher event when the file still has the materialized content hash.
    pub fn consume_if_expected(&self, path: impl AsRef<Path>) -> Result<bool, ProjectionError> {
        let path = path.as_ref();
        let key = path_key(path);
        let actual = match std::fs::read(path) {
            Ok(bytes) => Some(blake3::hash(&bytes).to_hex().to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let mut hashes = self.hashes.lock().map_err(|_| ProjectionError::Poisoned)?;
        let matched = hashes.get(&key).is_some_and(|expected| {
            actual
                .as_ref()
                .is_some_and(|actual_hash| actual_hash == expected)
        });
        if hashes.remove(&key).is_some() {
            self.persist(&hashes)?;
        }
        Ok(matched)
    }

    fn record(&self, path: &Path, hash: String) -> Result<(), ProjectionError> {
        let mut hashes = self.hashes.lock().map_err(|_| ProjectionError::Poisoned)?;
        hashes.insert(path_key(path), hash);
        self.persist(&hashes)
    }

    fn remove(&self, path: &Path) -> Result<(), ProjectionError> {
        let mut hashes = self.hashes.lock().map_err(|_| ProjectionError::Poisoned)?;
        if hashes.remove(&path_key(path)).is_some() {
            self.persist(&hashes)?;
        }
        Ok(())
    }

    fn persist(&self, hashes: &BTreeMap<String, String>) -> Result<(), ProjectionError> {
        let Some(state_path) = &self.state_path else {
            return Ok(());
        };
        let parent = state_path
            .parent()
            .ok_or_else(|| ProjectionError::InvalidPath(state_path.display().to_string()))?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, hashes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(state_path)?;
        Ok(())
    }
}

pub fn scan(root: impl AsRef<Path>) -> Result<ScanReport, ProjectionError> {
    scan_impl(root.as_ref(), false)
}

/// Scan a legacy workspace, assigning relocation-stable IDs in memory when they are absent.
///
/// The source files are read only. Generated IDs follow the legacy importer contract: the
/// first component encodes the note date and the remaining four characters come from SHA-256.
pub fn scan_for_import(root: impl AsRef<Path>) -> Result<ScanReport, ProjectionError> {
    scan_impl(root.as_ref(), true)
}

fn scan_impl(root: &Path, generate_missing_ids: bool) -> Result<ScanReport, ProjectionError> {
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
        let mut frontmatter = match document.frontmatter {
            Some(frontmatter) => frontmatter,
            None if generate_missing_ids => crate::domain::Frontmatter::new(),
            None => {
                report.diagnostics.push(Diagnostic {
                    path: relative,
                    code: "missing-frontmatter".to_owned(),
                    message: "Markdown document has no YAML frontmatter".to_owned(),
                });
                continue;
            }
        };
        let note_id = match frontmatter.get("id") {
            Some(FrontmatterValue::String(note_id)) if id::is_valid(note_id) => note_id.clone(),
            Some(FrontmatterValue::String(note_id)) if !generate_missing_ids => {
                report.diagnostics.push(Diagnostic {
                    path: relative,
                    code: "invalid-id".to_owned(),
                    message: format!("invalid note ID: {note_id}"),
                });
                continue;
            }
            Some(_) | None if !generate_missing_ids => {
                report.diagnostics.push(Diagnostic {
                    path: relative,
                    code: "missing-id".to_owned(),
                    message: "frontmatter has no string id".to_owned(),
                });
                continue;
            }
            _ => {
                let generated = deterministic_import_id(&frontmatter, &relative, &path)?;
                frontmatter.insert("id".to_owned(), FrontmatterValue::String(generated.clone()));
                generated
            }
        };
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

fn deterministic_import_id(
    frontmatter: &crate::domain::Frontmatter,
    relative: &str,
    path: &Path,
) -> Result<String, ProjectionError> {
    let timestamp = ["created", "added"]
        .into_iter()
        .find_map(|key| match frontmatter.get(key) {
            Some(FrontmatterValue::String(value)) => parse_timestamp(value),
            _ => None,
        })
        .unwrap_or(path.metadata()?.modified()?.into());
    let mut id = id::encode_base32(id::days_since_epoch(timestamp));
    let digest = Sha256::digest(relative.as_bytes());
    let mut seed = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 has eight bytes"));
    for _ in 0..4 {
        id.push(char::from(id::BASE32_CHARS[(seed % 32) as usize]));
        seed /= 32;
    }
    if id.len() < 7 {
        id = format!("{id:0>7}");
    }
    Ok(id)
}

fn parse_timestamp(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok().or_else(|| {
        Date::parse(value, format_description!("[year]-[month]-[day]"))
            .ok()
            .map(|date| date.midnight().assume_utc())
    })
}

pub fn materialize(root: impl AsRef<Path>, note: &Note) -> Result<PathBuf, ProjectionError> {
    let root = root.as_ref();
    let destination = safe_join(root, &note.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| ProjectionError::InvalidPath(note.path.clone()))?;
    std::fs::create_dir_all(parent)?;
    let content = markdown::render(&note.frontmatter, &note.body)?;
    write_content(&destination, parent, content.as_bytes())?;
    Ok(destination)
}

/// Atomically materialize a note and durably mark the resulting watcher event as expected.
pub fn materialize_expected(
    root: impl AsRef<Path>,
    note: &Note,
    expected_writes: &ExpectedWrites,
) -> Result<PathBuf, ProjectionError> {
    let root = root.as_ref();
    let destination = safe_join(root, &note.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| ProjectionError::InvalidPath(note.path.clone()))?;
    std::fs::create_dir_all(parent)?;
    let content = markdown::render(&note.frontmatter, &note.body)?;
    expected_writes.record(
        &destination,
        blake3::hash(content.as_bytes()).to_hex().to_string(),
    )?;
    if let Err(error) = write_content(&destination, parent, content.as_bytes()) {
        expected_writes.remove(&destination)?;
        return Err(error);
    }
    Ok(destination)
}

fn write_content(destination: &Path, parent: &Path, content: &[u8]) -> Result<(), ProjectionError> {
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    temporary.persist(destination)?;
    Ok(())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

    #[test]
    fn expected_materialization_survives_restart_and_suppresses_one_event() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join(".exo/expected-writes.json");
        let expected = ExpectedWrites::open(&state_path).unwrap();
        let destination = materialize_expected(
            directory.path(),
            &note("note002", "notes/one.md"),
            &expected,
        )
        .unwrap();
        drop(expected);

        let restored = ExpectedWrites::open(&state_path).unwrap();
        assert!(restored.consume_if_expected(&destination).unwrap());
        assert!(!restored.consume_if_expected(&destination).unwrap());
    }

    #[test]
    fn locally_changed_content_is_not_suppressed() {
        let directory = tempfile::tempdir().unwrap();
        let expected = ExpectedWrites::default();
        let destination = materialize_expected(
            directory.path(),
            &note("note002", "notes/one.md"),
            &expected,
        )
        .unwrap();
        std::fs::write(&destination, "local edit").unwrap();
        assert!(!expected.consume_if_expected(destination).unwrap());
    }

    #[test]
    fn legacy_import_assigns_a_stable_id_without_modifying_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.md");
        let content = "---\nadded: 2020-01-02T00:00:00Z\ntitle: Legacy\n---\n\nbody\n";
        std::fs::write(&path, content).unwrap();
        let first = scan_for_import(directory.path()).unwrap();
        let second = scan_for_import(directory.path()).unwrap();
        assert_eq!(first, second);
        assert!(id::is_valid(first.notes[0].id.as_str()));
        assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    }
}
