use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use xo_core::domain::FrontmatterValue;
use xo_core::{Note, NoteId};

use crate::session::WorkspaceSession;

const INTERNAL_EXPORT_FIELDS: [&str; 3] = ["id", "type", "created"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportResult {
    pub exported: usize,
    pub paths: Vec<PathBuf>,
}

pub async fn import_markdown(
    session: &mut WorkspaceSession,
    source: &Path,
    default_type: &str,
) -> Result<usize> {
    let source = source
        .canonicalize()
        .with_context(|| format!("resolve import source {}", source.display()))?;
    if !source.is_dir() {
        bail!("import source {} must be a directory", source.display());
    }
    let projection = session.projection_root();
    if source.starts_with(projection) || projection.starts_with(&source) {
        bail!(
            "import source {} and active projection {} must not overlap",
            source.display(),
            projection.display()
        );
    }
    let existing = session.snapshot().await?.notes;
    let notes = prepare_import(&source, &existing, default_type)?;
    for note in &notes {
        session.save(note).await?;
    }
    session.snapshot().await?;
    Ok(notes.len())
}

pub async fn export_markdown(
    session: &WorkspaceSession,
    destination: &Path,
    type_filter: Option<&str>,
) -> Result<ExportResult> {
    let notes = session.snapshot().await?.notes;
    export_notes(&notes, destination, type_filter)
}

pub fn prepare_import(source: &Path, existing: &[Note], default_type: &str) -> Result<Vec<Note>> {
    if default_type.trim().is_empty() {
        bail!("default import type must not be empty");
    }
    let report = xo_core::projection::scan_for_import(source)?;
    if !report.diagnostics.is_empty() {
        let details = report
            .diagnostics
            .iter()
            .map(|diagnostic| {
                format!(
                    "{} [{}]: {}",
                    diagnostic.path, diagnostic.code, diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "import source has {} diagnostic(s); nothing was imported\n{details}",
            report.diagnostics.len()
        );
    }

    let existing_ids = existing
        .iter()
        .map(|note| (note.id.clone(), note.path.as_str()))
        .collect::<BTreeMap<_, _>>();
    let existing_paths = existing
        .iter()
        .map(|note| (note.path.as_str(), &note.id))
        .collect::<BTreeMap<_, _>>();
    let mut notes = report.notes;
    for note in &mut notes {
        if let Some(path) = existing_ids.get(&note.id) {
            bail!(
                "import note {} conflicts with existing note ID {} at {path}; nothing was imported",
                note.path,
                note.id
            );
        }
        normalize_imported_note(note, default_type)?;
        if let Some(id) = existing_paths.get(note.path.as_str()) {
            bail!(
                "import path {} conflicts with existing note {id}; nothing was imported",
                note.path
            );
        }
    }
    Ok(notes)
}

pub fn export_notes(
    notes: &[Note],
    destination: &Path,
    type_filter: Option<&str>,
) -> Result<ExportResult> {
    ensure_empty_destination(destination)?;
    let mut notes = notes
        .iter()
        .filter(|note| type_filter.is_none_or(|filter| note_type(note) == filter))
        .collect::<Vec<_>>();
    notes.sort_by_key(|note| export_sort_key(note));

    let mut allocated = BTreeSet::new();
    let mut files = Vec::new();
    for note in notes {
        note.frontmatter
            .values()
            .try_for_each(FrontmatterValue::validate)
            .with_context(|| format!("validate note {}", note.id))?;
        let item_type = safe_path_component(note_type(note), "note");
        let (year, month) = export_year_month(note);
        let title = note_title(note);
        let slug = safe_path_component(&xo_core::markdown::slugify(title), "untitled");
        let directory = PathBuf::from(item_type).join(year).join(month);
        let relative = allocate_export_path(&directory, &slug, &mut allocated);
        let mut frontmatter = note.frontmatter.clone();
        for field in INTERNAL_EXPORT_FIELDS {
            frontmatter.remove(field);
        }
        if xo_core::encryption::is_encrypted(&note.body) {
            frontmatter.insert(
                "id".to_owned(),
                FrontmatterValue::String(note.id.to_string()),
            );
        }
        let content = xo_core::markdown::render(&frontmatter, &note.body)
            .with_context(|| format!("render note {}", note.id))?;
        files.push((relative, content));
    }

    std::fs::create_dir_all(destination)
        .with_context(|| format!("create export destination {}", destination.display()))?;
    let mut paths = Vec::with_capacity(files.len());
    for (relative, content) in files {
        let path = destination.join(relative);
        let parent = path
            .parent()
            .context("export path has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create export directory {}", parent.display()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&path)
            .with_context(|| format!("persist exported note {}", path.display()))?;
        paths.push(path);
    }
    Ok(ExportResult {
        exported: paths.len(),
        paths,
    })
}

fn normalize_imported_note(note: &mut Note, default_type: &str) -> Result<()> {
    note.frontmatter.insert(
        "id".to_owned(),
        FrontmatterValue::String(note.id.to_string()),
    );
    if !matches!(
        note.frontmatter.get("type"),
        Some(FrontmatterValue::String(value)) if !value.trim().is_empty()
    ) {
        note.frontmatter.insert(
            "type".to_owned(),
            FrontmatterValue::String(default_type.to_owned()),
        );
    }
    if !matches!(
        note.frontmatter.get("tags"),
        Some(FrontmatterValue::String(_) | FrontmatterValue::Sequence(_))
    ) {
        note.frontmatter
            .insert("tags".to_owned(), FrontmatterValue::Sequence(Vec::new()));
    }
    if !matches!(
        note.frontmatter.get("title"),
        Some(FrontmatterValue::String(value)) if !value.trim().is_empty()
    ) {
        let title = Path::new(&note.path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Untitled")
            .to_owned();
        note.frontmatter
            .insert("title".to_owned(), FrontmatterValue::String(title));
    }
    if !matches!(
        note.frontmatter.get("created"),
        Some(FrontmatterValue::String(value)) if !value.trim().is_empty()
    ) {
        let created = note
            .frontmatter
            .get("added")
            .and_then(|value| frontmatter_string(Some(value)))
            .map(str::to_owned)
            .map_or_else(
                || {
                    OffsetDateTime::now_utc()
                        .format(&Rfc3339)
                        .context("format import timestamp")
                },
                Ok,
            )?;
        note.frontmatter
            .insert("created".to_owned(), FrontmatterValue::String(created));
    }
    note.frontmatter
        .values()
        .try_for_each(FrontmatterValue::validate)
        .with_context(|| format!("validate imported note {}", note.path))?;
    note.path = xo_core::projection::canonical_note_path(&note.id, &note.frontmatter);
    Ok(())
}

fn ensure_empty_destination(destination: &Path) -> Result<()> {
    match std::fs::metadata(destination) {
        Ok(metadata) if !metadata.is_dir() => {
            bail!(
                "export destination {} is not a directory",
                destination.display()
            );
        }
        Ok(_) if std::fs::read_dir(destination)?.next().is_some() => {
            bail!(
                "export destination {} is not empty; refusing to overwrite files",
                destination.display()
            );
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn allocate_export_path(
    directory: &Path,
    slug: &str,
    allocated: &mut BTreeSet<PathBuf>,
) -> PathBuf {
    for suffix in 0_u64.. {
        let filename = if suffix == 0 {
            format!("{slug}.md")
        } else {
            format!("{slug}-{suffix}.md")
        };
        let candidate = directory.join(filename);
        if allocated.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the export filename suffix space is unbounded")
}

fn export_sort_key(note: &Note) -> (String, String, String, NoteId) {
    (
        note_type(note).to_owned(),
        frontmatter_string(note.frontmatter.get("created"))
            .unwrap_or_default()
            .to_owned(),
        note_title(note).to_owned(),
        note.id.clone(),
    )
}

fn export_year_month(note: &Note) -> (String, String) {
    let Some(created) = frontmatter_string(note.frontmatter.get("created")) else {
        return ("unknown".to_owned(), "unknown".to_owned());
    };
    let bytes = created.as_bytes();
    if bytes.len() >= 7
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
    {
        (created[0..4].to_owned(), created[5..7].to_owned())
    } else {
        ("unknown".to_owned(), "unknown".to_owned())
    }
}

fn note_type(note: &Note) -> &str {
    frontmatter_string(note.frontmatter.get("type")).unwrap_or("note")
}

fn note_title(note: &Note) -> &str {
    frontmatter_string(note.frontmatter.get("title")).unwrap_or_else(|| note.id.as_str())
}

fn frontmatter_string(value: Option<&FrontmatterValue>) -> Option<&str> {
    match value {
        Some(FrontmatterValue::String(value)) => Some(value),
        _ => None,
    }
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let slug = xo_core::markdown::slugify(value);
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xo_core::domain::Frontmatter;

    #[test]
    fn recursive_import_normalizes_required_fields_without_touching_source() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source");
        std::fs::create_dir_all(source.join("nested"))?;
        let raw = "# Plain body\n";
        let existing = "---\ntitle: Existing\nadded: 2024-03-02\n---\nBody\n";
        std::fs::write(source.join("plain.md"), raw)?;
        std::fs::write(source.join("nested/existing.md"), existing)?;

        let notes = prepare_import(&source, &[], "note")?;

        assert_eq!(notes.len(), 2);
        for note in &notes {
            for field in ["id", "created", "tags", "title", "type"] {
                assert!(note.frontmatter.contains_key(field), "{field} missing");
            }
        }
        assert_eq!(std::fs::read_to_string(source.join("plain.md"))?, raw);
        assert_eq!(
            std::fs::read_to_string(source.join("nested/existing.md"))?,
            existing
        );
        Ok(())
    }

    #[test]
    fn import_preflight_rejects_diagnostics_and_existing_collisions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let malformed = directory.path().join("malformed");
        std::fs::create_dir(&malformed)?;
        std::fs::write(malformed.join("bad.md"), "---\ntitle: [\n---\n")?;
        let error = prepare_import(&malformed, &[], "note")
            .unwrap_err()
            .to_string();
        assert!(error.contains("nothing was imported"));
        assert!(error.contains("malformed-markdown"));

        let source = directory.path().join("collision");
        std::fs::create_dir(&source)?;
        std::fs::write(
            source.join("same.md"),
            "---\nid: note002\ntitle: Imported\n---\n",
        )?;
        let existing = vec![note("note002", "elsewhere.md", "Existing", "note")];
        let error = prepare_import(&source, &existing, "note")
            .unwrap_err()
            .to_string();
        assert!(error.contains("conflicts with existing note ID"));
        Ok(())
    }

    #[test]
    fn export_filters_strips_internal_fields_and_resolves_collisions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("export");
        let mut first = note("note001", "a.md", "Same title", "note");
        first.frontmatter.insert(
            "created".into(),
            FrontmatterValue::String("2025-04-01T00:00:00Z".into()),
        );
        let mut second = note("note002", "b.md", "Same title", "note");
        second.frontmatter.insert(
            "created".into(),
            FrontmatterValue::String("2025-04-02T00:00:00Z".into()),
        );
        let book = note("note003", "book.md", "Book", "book");

        let result = export_notes(&[second, book, first], &destination, Some("note"))?;

        assert_eq!(result.exported, 2);
        assert!(destination.join("note/2025/04/same-title.md").is_file());
        assert!(destination.join("note/2025/04/same-title-1.md").is_file());
        let exported = std::fs::read_to_string(destination.join("note/2025/04/same-title.md"))?;
        let parsed = xo_core::markdown::parse(&exported)?;
        let frontmatter = parsed.frontmatter.context("exported frontmatter")?;
        assert!(!frontmatter.contains_key("id"));
        assert!(!frontmatter.contains_key("type"));
        assert!(!frontmatter.contains_key("created"));
        assert_eq!(parsed.body, "Body\n");
        Ok(())
    }

    #[test]
    fn export_preserves_encrypted_id_and_refuses_nonempty_destination() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("export");
        let mut encrypted = note("note001", "secret.md", "Secret", "note");
        encrypted.body = xo_core::encryption::encrypt("note001", "passphrase", "secret")?;
        export_notes(std::slice::from_ref(&encrypted), &destination, None)?;
        let path = destination.join("note/unknown/unknown/secret.md");
        let parsed = xo_core::markdown::parse(&std::fs::read_to_string(path)?)?;
        assert_eq!(
            parsed
                .frontmatter
                .context("encrypted export frontmatter")?
                .get("id"),
            Some(&FrontmatterValue::String("note001".into()))
        );
        let error = export_notes(&[encrypted], &destination, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is not empty"));
        Ok(())
    }

    fn note(id: &str, path: &str, title: &str, item_type: &str) -> Note {
        Note {
            id: NoteId::new(id),
            frontmatter: Frontmatter::from([
                ("id".into(), FrontmatterValue::String(id.into())),
                ("title".into(), FrontmatterValue::String(title.into())),
                ("type".into(), FrontmatterValue::String(item_type.into())),
                ("tags".into(), FrontmatterValue::Sequence(Vec::new())),
            ]),
            body: "Body\n".into(),
            path: path.into(),
        }
    }
}
