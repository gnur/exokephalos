use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime};

use crate::domain::{AssetRecord, FrontmatterValue, Note};
use crate::{NoteId, id, markdown};

const DELETED_HASH: &str = "deleted";

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaterializationReport {
    pub materialized: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedAsset {
    pub record: AssetRecord,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("projection I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid projection path: {0}")]
    InvalidPath(String),
    #[error("invalid projected note {path}: {message}")]
    InvalidNote { path: String, message: String },
    #[error("invalid projected asset {path}: {message}")]
    InvalidAsset { path: String, message: String },
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
        let matched = hashes.get(&key).is_some_and(|expected| match &actual {
            Some(actual_hash) => actual_hash == expected,
            None => expected == DELETED_HASH,
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

    fn record_deletion(&self, path: &Path) -> Result<(), ProjectionError> {
        self.record(path, DELETED_HASH.to_owned())
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MaterializedEntry {
    path: String,
    hash: String,
}

#[derive(Debug)]
struct DesiredFile {
    id: String,
    path: String,
    bytes: Vec<u8>,
}

/// Durable state for projecting authoritative winning heads into a Markdown tree.
#[derive(Debug)]
pub struct ProjectionState {
    root: PathBuf,
    manifest_path: PathBuf,
    asset_manifest_path: PathBuf,
    expected_writes: ExpectedWrites,
}

impl ProjectionState {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectionError> {
        std::fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        let state_dir = root.join(".exo");
        Ok(Self {
            manifest_path: state_dir.join("projection.json"),
            asset_manifest_path: state_dir.join("assets.json"),
            expected_writes: ExpectedWrites::open(state_dir.join("expected-writes.json"))?,
            root,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn consume_if_expected(&self, path: impl AsRef<Path>) -> Result<bool, ProjectionError> {
        self.expected_writes.consume_if_expected(path)
    }

    /// Reconcile the projection with resolved visible notes without overwriting local edits.
    pub fn reconcile(&self, notes: &[Note]) -> Result<MaterializationReport, ProjectionError> {
        let mut desired = Vec::new();
        let mut diagnostics = Vec::new();
        for note in notes {
            if is_asset_path(&note.path) {
                diagnostics.push(Diagnostic {
                    path: note.path.clone(),
                    code: "reserved-asset-path".to_owned(),
                    message: "note paths cannot be below assets/".to_owned(),
                });
                continue;
            }
            desired.push(DesiredFile {
                id: note.id.to_string(),
                path: note.path.clone(),
                bytes: markdown::render(&note.frontmatter, &note.body)?.into_bytes(),
            });
        }
        let mut report = reconcile_files(
            &self.root,
            &self.manifest_path,
            &self.expected_writes,
            desired,
        )?;
        report.diagnostics.extend(diagnostics);
        Ok(report)
    }

    /// Reconcile verified binary assets below the reserved `assets/` directory.
    pub fn reconcile_assets(
        &self,
        assets: &[ProjectedAsset],
    ) -> Result<MaterializationReport, ProjectionError> {
        let mut desired = Vec::new();
        for asset in assets {
            asset
                .record
                .validate()
                .map_err(|error| ProjectionError::InvalidAsset {
                    path: asset.record.materialized_path.clone(),
                    message: error.to_string(),
                })?;
            let size_matches = u64::try_from(asset.bytes.len()).ok() == Some(asset.record.size);
            let hash_matches =
                blake3::hash(&asset.bytes).to_hex().as_str() == asset.record.blob_hash;
            if !size_matches || !hash_matches {
                return Err(ProjectionError::InvalidAsset {
                    path: asset.record.materialized_path.clone(),
                    message: "bytes do not match the declared hash and size".to_owned(),
                });
            }
            desired.push(DesiredFile {
                id: asset.record.id.to_string(),
                path: asset.record.materialized_path.clone(),
                bytes: asset.bytes.clone(),
            });
        }
        reconcile_files(
            &self.root,
            &self.asset_manifest_path,
            &self.expected_writes,
            desired,
        )
    }
}

fn reconcile_files(
    root: &Path,
    manifest_path: &Path,
    expected_writes: &ExpectedWrites,
    desired: Vec<DesiredFile>,
) -> Result<MaterializationReport, ProjectionError> {
    let mut report = MaterializationReport::default();
    let desired = unique_desired(desired, &mut report.diagnostics);
    let mut manifest = load_manifest(manifest_path)?;
    remove_obsolete(root, &desired, &mut manifest, expected_writes, &mut report)?;
    for file in desired.values() {
        materialize_desired(root, file, &mut manifest, expected_writes, &mut report)?;
    }
    persist_manifest(manifest_path, &manifest)?;
    Ok(report)
}

fn unique_desired(
    desired: Vec<DesiredFile>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<String, DesiredFile> {
    let mut result = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for file in desired {
        if result.contains_key(&file.id) || !paths.insert(file.path.clone()) {
            diagnostics.push(Diagnostic {
                path: file.path,
                code: "projection-path-conflict".to_owned(),
                message: "multiple records resolve to this identity or path".to_owned(),
            });
        } else {
            result.insert(file.id.clone(), file);
        }
    }
    result
}

fn remove_obsolete(
    root: &Path,
    desired: &BTreeMap<String, DesiredFile>,
    manifest: &mut BTreeMap<String, MaterializedEntry>,
    expected_writes: &ExpectedWrites,
    report: &mut MaterializationReport,
) -> Result<(), ProjectionError> {
    let obsolete = manifest
        .iter()
        .filter(|(id, entry)| desired.get(*id).is_none_or(|file| file.path != entry.path))
        .map(|(id, entry)| (id.clone(), entry.clone()))
        .collect::<Vec<_>>();
    for (id, entry) in obsolete {
        let path = safe_join(root, &entry.path)?;
        match file_hash(&path)? {
            None => {
                manifest.remove(&id);
            }
            Some(hash) if hash == entry.hash => {
                expected_writes.record_deletion(&path)?;
                if let Err(error) = std::fs::remove_file(&path) {
                    expected_writes.remove(&path)?;
                    return Err(error.into());
                }
                manifest.remove(&id);
                report.removed.push(path);
            }
            Some(_) => report.diagnostics.push(Diagnostic {
                path: entry.path,
                code: "local-edit-preserved".to_owned(),
                message: "obsolete projection differs from its last materialized hash".to_owned(),
            }),
        }
    }
    Ok(())
}

fn materialize_desired(
    root: &Path,
    file: &DesiredFile,
    manifest: &mut BTreeMap<String, MaterializedEntry>,
    expected_writes: &ExpectedWrites,
    report: &mut MaterializationReport,
) -> Result<(), ProjectionError> {
    let destination = safe_join(root, &file.path)?;
    let desired_hash = blake3::hash(&file.bytes).to_hex().to_string();
    let actual_hash = file_hash(&destination)?;
    let prior = manifest.get(&file.id);
    let safe_to_write = actual_hash.is_none()
        || actual_hash.as_ref() == Some(&desired_hash)
        || prior.is_some_and(|entry| {
            entry.path == file.path && actual_hash.as_ref() == Some(&entry.hash)
        });
    if !safe_to_write {
        report.diagnostics.push(Diagnostic {
            path: file.path.clone(),
            code: "local-edit-preserved".to_owned(),
            message: "file differs from the last materialized version".to_owned(),
        });
        return Ok(());
    }
    if actual_hash.as_ref() != Some(&desired_hash) {
        expected_writes.record(&destination, desired_hash.clone())?;
        let parent = destination
            .parent()
            .ok_or_else(|| ProjectionError::InvalidPath(file.path.clone()))?;
        std::fs::create_dir_all(parent)?;
        if let Err(error) = write_content(&destination, parent, &file.bytes) {
            expected_writes.remove(&destination)?;
            return Err(error);
        }
        report.materialized.push(destination);
    }
    manifest.insert(
        file.id.clone(),
        MaterializedEntry {
            path: file.path.clone(),
            hash: desired_hash,
        },
    );
    Ok(())
}

fn load_manifest(path: &Path) -> Result<BTreeMap<String, MaterializedEntry>, ProjectionError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn persist_manifest(
    path: &Path,
    manifest: &BTreeMap<String, MaterializedEntry>,
) -> Result<(), ProjectionError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProjectionError::InvalidPath(path.display().to_string()))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, manifest)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

fn file_hash(path: &Path) -> Result<Option<String>, ProjectionError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(blake3::hash(&bytes).to_hex().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn scan(root: impl AsRef<Path>) -> Result<ScanReport, ProjectionError> {
    scan_impl(root.as_ref(), false)
}

/// Read and validate one Markdown file relative to a projection root.
pub fn read_note(root: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<Note, ProjectionError> {
    let root = root.as_ref();
    let path = path.as_ref();
    let relative = relative_string(root, path)?;
    safe_join(root, &relative)?;
    if is_asset_path(&relative) {
        return Err(ProjectionError::InvalidNote {
            path: relative,
            message: "note paths cannot be below assets/".to_owned(),
        });
    }
    let content = std::fs::read_to_string(path)?;
    let document = markdown::parse(&content)?;
    let frontmatter = document
        .frontmatter
        .ok_or_else(|| ProjectionError::InvalidNote {
            path: relative.clone(),
            message: "Markdown document has no YAML frontmatter".to_owned(),
        })?;
    let note_id = match frontmatter.get("id") {
        Some(FrontmatterValue::String(note_id)) if id::is_valid(note_id) => note_id.clone(),
        Some(FrontmatterValue::String(note_id)) => {
            return Err(ProjectionError::InvalidNote {
                path: relative,
                message: format!("invalid note ID: {note_id}"),
            });
        }
        _ => {
            return Err(ProjectionError::InvalidNote {
                path: relative,
                message: "frontmatter has no string id".to_owned(),
            });
        }
    };
    Ok(Note {
        id: NoteId::new(note_id),
        frontmatter,
        body: document.body,
        path: relative,
    })
}

pub fn relative_path(
    root: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> Result<String, ProjectionError> {
    relative_string(root.as_ref(), path.as_ref())
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
            if relative
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == "assets")
                || relative.components().any(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .is_some_and(|name| name.starts_with('.'))
                })
            {
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

fn is_asset_path(relative: &str) -> bool {
    Path::new(relative)
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == "assets")
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

    #[test]
    fn reconciliation_materializes_removes_and_preserves_local_edits() {
        let directory = tempfile::tempdir().unwrap();
        let state = ProjectionState::open(directory.path()).unwrap();
        let original = note("note002", "deep/original.md");
        let first = state.reconcile(std::slice::from_ref(&original)).unwrap();
        let path = state.root().join(&original.path);
        assert_eq!(first.materialized, vec![path.clone()]);
        assert!(state.consume_if_expected(&path).unwrap());

        std::fs::write(&path, "unprocessed local edit").unwrap();
        let mut remote = original.clone();
        remote.body = "remote body\n".to_owned();
        let preserved = state.reconcile(std::slice::from_ref(&remote)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "unprocessed local edit"
        );
        assert_eq!(preserved.diagnostics[0].code, "local-edit-preserved");

        materialize(directory.path(), &original).unwrap();
        let updated = state.reconcile(std::slice::from_ref(&remote)).unwrap();
        assert_eq!(updated.materialized, vec![path.clone()]);
        assert!(state.consume_if_expected(&path).unwrap());
        let removed = state.reconcile(&[]).unwrap();
        assert_eq!(removed.removed, vec![path.clone()]);
        assert!(!path.exists());
        assert!(state.consume_if_expected(path).unwrap());
    }

    #[test]
    fn verified_assets_materialize_only_below_assets() {
        let directory = tempfile::tempdir().unwrap();
        let state = ProjectionState::open(directory.path()).unwrap();
        let bytes = b"png bytes".to_vec();
        let asset = ProjectedAsset {
            record: AssetRecord {
                schema: crate::CURRENT_SCHEMA,
                id: crate::AssetId::new("image001"),
                blob_hash: blake3::hash(&bytes).to_hex().to_string(),
                mime: "image/png".to_owned(),
                size: bytes.len() as u64,
                materialized_path: "assets/images/example.png".to_owned(),
            },
            bytes: bytes.clone(),
        };
        let report = state
            .reconcile_assets(std::slice::from_ref(&asset))
            .unwrap();
        let path = state.root().join(&asset.record.materialized_path);
        assert_eq!(report.materialized, vec![path.clone()]);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        let mut outside = asset;
        outside.record.materialized_path = "images/example.png".to_owned();
        assert!(matches!(
            state.reconcile_assets(&[outside]),
            Err(ProjectionError::InvalidAsset { .. })
        ));
    }
}
