use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, params};
use thiserror::Error;

use crate::domain::FrontmatterValue;
use crate::projection::Diagnostic;
use crate::{Note, NoteId};

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("SQLite index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("local index lock was poisoned")]
    Poisoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedNote {
    pub id: NoteId,
    pub path: String,
    pub title: String,
    pub note_type: String,
    pub tags: Vec<String>,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct LocalIndex {
    connection: Mutex<Connection>,
}

impl LocalIndex {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, IndexError> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS notes (
                 id TEXT PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 title TEXT NOT NULL,
                 note_type TEXT NOT NULL,
                 tags_json TEXT NOT NULL,
                 content_hash TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS diagnostics (
                 path TEXT NOT NULL,
                 code TEXT NOT NULL,
                 message TEXT NOT NULL,
                 PRIMARY KEY(path, code, message)
             );",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn rebuild(&self, notes: &[Note], diagnostics: &[Diagnostic]) -> Result<(), IndexError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM notes", [])?;
        transaction.execute("DELETE FROM diagnostics", [])?;
        for note in notes {
            let indexed = indexed_note(note);
            transaction.execute(
                "INSERT INTO notes(id, path, title, note_type, tags_json, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    indexed.id.as_str(),
                    indexed.path,
                    indexed.title,
                    indexed.note_type,
                    serde_json::to_string(&indexed.tags).expect("string tags serialize"),
                    indexed.content_hash,
                ],
            )?;
        }
        for diagnostic in diagnostics {
            transaction.execute(
                "INSERT INTO diagnostics(path, code, message) VALUES (?1, ?2, ?3)",
                params![diagnostic.path, diagnostic.code, diagnostic.message],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<IndexedNote>, IndexError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, path, title, note_type, tags_json, content_hash FROM notes ORDER BY path",
        )?;
        let rows = statement.query_map([], |row| {
            let tags_json: String = row.get(4)?;
            let tags = serde_json::from_str(&tags_json).unwrap_or_default();
            Ok(IndexedNote {
                id: NoteId::new(row.get::<_, String>(0)?),
                path: row.get(1)?,
                title: row.get(2)?,
                note_type: row.get(3)?,
                tags,
                content_hash: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    pub fn diagnostics(&self) -> Result<Vec<Diagnostic>, IndexError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT path, code, message FROM diagnostics ORDER BY path, code")?;
        let rows = statement.query_map([], |row| {
            Ok(Diagnostic {
                path: row.get(0)?,
                code: row.get(1)?,
                message: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, IndexError> {
        self.connection.lock().map_err(|_| IndexError::Poisoned)
    }
}

fn indexed_note(note: &Note) -> IndexedNote {
    let string = |key: &str| match note.frontmatter.get(key) {
        Some(FrontmatterValue::String(value)) => value.clone(),
        Some(FrontmatterValue::Integer(value)) => value.to_string(),
        _ => String::new(),
    };
    let configured_title = string("title");
    let title = if configured_title.is_empty() {
        std::path::Path::new(&note.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned()
    } else {
        configured_title
    };
    let tags = crate::markdown::tags(&note.frontmatter)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let rendered =
        crate::markdown::render(&note.frontmatter, &note.body).expect("parsed frontmatter renders");
    IndexedNote {
        id: note.id.clone(),
        path: note.path.clone(),
        title,
        note_type: string("type"),
        tags,
        content_hash: blake3::hash(rendered.as_bytes()).to_hex().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{Frontmatter, FrontmatterValue};

    use super::*;

    #[test]
    fn rebuild_replaces_notes_and_diagnostics_transactionally() {
        let directory = tempfile::tempdir().unwrap();
        let index = LocalIndex::open(directory.path().join("index.sqlite")).unwrap();
        let note = Note {
            id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([
                (
                    "title".to_owned(),
                    FrontmatterValue::String("Indexed".to_owned()),
                ),
                (
                    "type".to_owned(),
                    FrontmatterValue::String("note".to_owned()),
                ),
                (
                    "tags".to_owned(),
                    FrontmatterValue::Sequence(vec![FrontmatterValue::String("rust".to_owned())]),
                ),
            ]),
            body: "body".to_owned(),
            path: "notes/indexed.md".to_owned(),
        };
        let diagnostic = Diagnostic {
            path: "broken.md".to_owned(),
            code: "malformed-markdown".to_owned(),
            message: "invalid YAML".to_owned(),
        };
        index
            .rebuild(
                std::slice::from_ref(&note),
                std::slice::from_ref(&diagnostic),
            )
            .unwrap();
        let indexed = index.all().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].title, "Indexed");
        assert_eq!(indexed[0].tags, vec!["rust"]);
        assert_eq!(index.diagnostics().unwrap(), vec![diagnostic]);

        index.rebuild(&[], &[]).unwrap();
        assert!(index.all().unwrap().is_empty());
        assert!(index.diagnostics().unwrap().is_empty());
    }
}
