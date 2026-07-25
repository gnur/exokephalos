//! Bidirectional bridge between authoritative records and a local Markdown projection.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::iroh_node::IrohWorkspace;
use crate::local_index::LocalIndex;
use crate::projection::{
    Diagnostic, MaterializationReport, ProjectionError, ProjectionState, read_note, relative_path,
};
use crate::records::{RecordError, WorkspaceRecords, WorkspaceSnapshot};
use crate::watcher::ProjectionEvent;
use crate::{ActorId, CURRENT_SCHEMA, HlcClock, Note, NoteRevision, RevisionId};

#[derive(Debug, Error)]
pub enum WorkspaceProjectionError {
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Records(#[from] RecordError),
    #[error("projection clock lock was poisoned")]
    Poisoned,
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
    #[error("winning revision is unavailable: {0}")]
    MissingWinningRevision(RevisionId),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefreshReport {
    pub snapshot: WorkspaceSnapshot,
    pub materialization: MaterializationReport,
    pub asset_materialization: MaterializationReport,
    pub config_materialization: MaterializationReport,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalApplyReport {
    pub committed: Vec<RevisionId>,
    pub suppressed: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub refresh: RefreshReport,
}

/// Serial projection pipeline for one local author and workspace.
#[derive(Debug)]
pub struct WorkspaceProjection<'a> {
    records: WorkspaceRecords<'a>,
    index: &'a LocalIndex,
    state: ProjectionState,
    actor: ActorId,
    clock: Mutex<HlcClock>,
}

impl<'a> WorkspaceProjection<'a> {
    pub fn open(
        workspace: &'a IrohWorkspace,
        index: &'a LocalIndex,
        root: impl AsRef<Path>,
    ) -> Result<Self, WorkspaceProjectionError> {
        let records = WorkspaceRecords::new(workspace);
        let actor = records.actor_id();
        Ok(Self {
            records,
            index,
            state: ProjectionState::open(root)?,
            clock: Mutex::new(HlcClock::new(actor.clone())),
            actor,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.state.root()
    }

    /// Rebuild the derived index and safely materialize every resolved winning head.
    pub async fn refresh(&self) -> Result<RefreshReport, WorkspaceProjectionError> {
        let snapshot = self.records.rebuild_index(self.index).await?;
        let materialization = self.state.reconcile(&snapshot.notes)?;
        let asset_materialization = self.state.reconcile_assets(&snapshot.assets)?;
        let config_materialization = self.state.reconcile_configs(&snapshot.configs)?;
        Ok(RefreshReport {
            snapshot,
            materialization,
            asset_materialization,
            config_materialization,
        })
    }

    /// Apply a debounced event batch. Upserts run before removals so renames form one revision.
    pub async fn apply_events(
        &self,
        events: &[ProjectionEvent],
    ) -> Result<LocalApplyReport, WorkspaceProjectionError> {
        let mut report = LocalApplyReport::default();
        let mut upserts = BTreeSet::<PathBuf>::new();
        let mut removals = BTreeSet::<PathBuf>::new();
        for event in events {
            let path = event.path();
            if self.state.consume_if_expected(path)? {
                report.suppressed += 1;
                continue;
            }
            match event {
                ProjectionEvent::Upsert(path) => {
                    upserts.insert(path.clone());
                }
                ProjectionEvent::Remove(path) => {
                    removals.insert(path.clone());
                }
            }
        }

        for path in upserts {
            if let Ok(relative) = relative_path(self.root(), &path)
                && is_config_path(&relative)
            {
                let bytes = std::fs::read(&path).map_err(ProjectionError::from)?;
                let current = self
                    .records
                    .list_configs()
                    .await?
                    .into_iter()
                    .find(|config| config.record.path == relative);
                if current.as_ref().is_none_or(|config| config.bytes != bytes) {
                    let predecessors = current.as_ref().map_or_else(BTreeSet::new, |config| {
                        BTreeSet::from([config.revision_id.clone()])
                    });
                    let timestamp = current.as_ref().map_or_else(
                        || self.next_timestamp(),
                        |config| self.next_after(&config.record.hlc),
                    )?;
                    report.committed.push(
                        self.records
                            .put_config(relative, bytes, timestamp, predecessors)
                            .await?
                            .revision_id,
                    );
                }
                continue;
            }
            match read_note(self.root(), &path) {
                Ok(note) => {
                    if let Some(revision_id) = self.commit_note(note).await? {
                        report.committed.push(revision_id);
                    }
                }
                Err(error) => report.diagnostics.push(local_diagnostic(&path, &error)),
            }
        }

        // Reload after upserts: a rename's new path is now authoritative, so removing its old
        // path cannot accidentally create a deletion revision.
        let snapshot = self.records.snapshot().await?;
        for path in removals {
            if path.exists() {
                continue;
            }
            let relative = match relative_path(self.root(), &path) {
                Ok(relative) => relative,
                Err(error) => {
                    report.diagnostics.push(local_diagnostic(&path, &error));
                    continue;
                }
            };
            let Some(base) = snapshot
                .resolved
                .iter()
                .filter_map(|resolved| resolved.visible.as_ref())
                .find(|revision| revision.materialized_path == relative)
            else {
                continue;
            };
            let timestamp = self.next_after(&base.hlc)?;
            let deletion = base.delete(timestamp, self.actor.clone())?;
            report
                .committed
                .push(self.records.commit_revision(&deletion).await?);
        }

        report.refresh = self.refresh().await?;
        report
            .diagnostics
            .extend(report.refresh.materialization.diagnostics.clone());
        report
            .diagnostics
            .extend(report.refresh.asset_materialization.diagnostics.clone());
        report
            .diagnostics
            .extend(report.refresh.config_materialization.diagnostics.clone());
        Ok(report)
    }

    async fn commit_note(
        &self,
        note: Note,
    ) -> Result<Option<RevisionId>, WorkspaceProjectionError> {
        let revision = if let Some(resolved) = self.records.load_note(&note.id).await? {
            let base = self
                .records
                .get_revision(&note.id, &resolved.winning_revision)
                .await?
                .ok_or_else(|| {
                    WorkspaceProjectionError::MissingWinningRevision(
                        resolved.winning_revision.clone(),
                    )
                })?;
            if !base.deleted
                && base.frontmatter == note.frontmatter
                && base.body == note.body
                && base.materialized_path == note.path
            {
                return Ok(None);
            }
            base.revise(
                note.frontmatter,
                note.body,
                note.path,
                self.next_after(&base.hlc)?,
                self.actor.clone(),
                false,
            )?
        } else {
            NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: note.id,
                frontmatter: note.frontmatter,
                body: note.body,
                materialized_path: note.path,
                hlc: self.next_timestamp()?,
                author_id: self.actor.clone(),
                predecessors: BTreeSet::new(),
                deleted: false,
            }
        };
        Ok(Some(self.records.commit_revision(&revision).await?))
    }

    fn next_timestamp(&self) -> Result<crate::Hlc, WorkspaceProjectionError> {
        let wall_clock_ms = wall_clock_ms()?;
        Ok(self.clock()?.next(wall_clock_ms))
    }

    fn next_after(&self, remote: &crate::Hlc) -> Result<crate::Hlc, WorkspaceProjectionError> {
        let wall_clock_ms = wall_clock_ms()?;
        Ok(self.clock()?.observe(remote, wall_clock_ms))
    }

    fn clock(&self) -> Result<MutexGuard<'_, HlcClock>, WorkspaceProjectionError> {
        self.clock
            .lock()
            .map_err(|_| WorkspaceProjectionError::Poisoned)
    }
}

fn is_config_path(path: &str) -> bool {
    matches!(path, "xo.scm" | "exo.scm")
        || (path.starts_with("modules/")
            && Path::new(path)
                .extension()
                .is_some_and(|value| value == "scm")
            && !path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
}

fn wall_clock_ms() -> Result<u64, WorkspaceProjectionError> {
    let wall_clock_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| WorkspaceProjectionError::InvalidSystemClock)?
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    Ok(wall_clock_ms)
}

fn local_diagnostic(path: &Path, error: &ProjectionError) -> Diagnostic {
    Diagnostic {
        path: path.to_string_lossy().into_owned(),
        code: "invalid-local-change".to_owned(),
        message: error.to_string(),
    }
}

impl From<crate::DomainError> for WorkspaceProjectionError {
    fn from(error: crate::DomainError) -> Self {
        Self::Records(RecordError::Domain(error))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::domain::{Frontmatter, FrontmatterValue};
    use crate::iroh_node::IrohNode;
    use crate::records::WorkspaceRecords;
    use crate::{Hlc, NoteId, SchemaVersion};

    use super::*;

    fn initial_revision(actor: ActorId) -> NoteRevision {
        NoteRevision {
            schema: SchemaVersion(1),
            note_id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([
                (
                    "id".to_owned(),
                    FrontmatterValue::String("note002".to_owned()),
                ),
                (
                    "title".to_owned(),
                    FrontmatterValue::String("Lifecycle".to_owned()),
                ),
            ]),
            body: "initial\n".to_owned(),
            materialized_path: "notes/initial.md".to_owned(),
            hlc: Hlc {
                physical_ms: 4_000_000_000_000,
                logical: 0,
                actor_id: actor.clone(),
            },
            author_id: actor,
            predecessors: BTreeSet::new(),
            deleted: false,
        }
    }

    #[tokio::test]
    async fn local_edit_rename_and_delete_become_one_revision_each() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let node = IrohNode::persistent(directory.path().join("iroh")).await?;
        let workspace = node.create_workspace().await?;
        let records = WorkspaceRecords::new(&workspace);
        let initial = initial_revision(records.actor_id());
        let initial_id = records.commit_revision(&initial).await?;
        let index = LocalIndex::open(directory.path().join("index.sqlite"))?;
        let projection_root = directory.path().join("projection");
        let projection = WorkspaceProjection::open(&workspace, &index, &projection_root)?;
        projection.refresh().await?;
        let initial_path = projection.root().join(&initial.materialized_path);

        let suppressed = projection
            .apply_events(&[ProjectionEvent::Upsert(initial_path.clone())])
            .await?;
        assert_eq!(suppressed.suppressed, 1);
        assert!(suppressed.committed.is_empty());
        assert_eq!(
            records
                .load_note(&initial.note_id)
                .await?
                .expect("resolved initial note")
                .winning_revision,
            initial_id
        );

        let mut edited = read_note(projection.root(), &initial_path)?;
        edited.body = "locally edited\n".to_owned();
        crate::projection::materialize(projection.root(), &edited)?;
        let edit = projection
            .apply_events(&[ProjectionEvent::Upsert(initial_path.clone())])
            .await?;
        assert_eq!(edit.committed.len(), 1);
        assert_eq!(
            records
                .load_note(&initial.note_id)
                .await?
                .expect("resolved edit")
                .visible
                .expect("visible edit")
                .body,
            "locally edited\n"
        );

        let renamed_path = projection.root().join("archive/renamed.md");
        std::fs::create_dir_all(renamed_path.parent().expect("rename parent"))?;
        std::fs::rename(&initial_path, &renamed_path)?;
        let rename = projection
            .apply_events(&[
                ProjectionEvent::Remove(initial_path),
                ProjectionEvent::Upsert(renamed_path.clone()),
            ])
            .await?;
        assert_eq!(rename.committed.len(), 1);
        assert_eq!(
            records
                .load_note(&initial.note_id)
                .await?
                .expect("resolved rename")
                .visible
                .expect("visible rename")
                .materialized_path,
            "archive/renamed.md"
        );

        std::fs::remove_file(&renamed_path)?;
        let deletion = projection
            .apply_events(&[ProjectionEvent::Remove(renamed_path)])
            .await?;
        assert_eq!(deletion.committed.len(), 1);
        assert!(
            records
                .load_note(&initial.note_id)
                .await?
                .expect("resolved deletion")
                .visible
                .is_none()
        );
        assert!(index.all()?.is_empty());
        node.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn local_create_becomes_an_initial_revision() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let node = IrohNode::persistent(directory.path().join("iroh")).await?;
        let workspace = node.create_workspace().await?;
        let records = WorkspaceRecords::new(&workspace);
        let index = LocalIndex::open(directory.path().join("index.sqlite"))?;
        let projection =
            WorkspaceProjection::open(&workspace, &index, directory.path().join("projection"))?;
        let created = Note {
            id: NoteId::new("new0002"),
            frontmatter: Frontmatter::from([(
                "id".to_owned(),
                FrontmatterValue::String("new0002".to_owned()),
            )]),
            body: "new note\n".to_owned(),
            path: "notes/new.md".to_owned(),
        };
        let path = crate::projection::materialize(projection.root(), &created)?;
        let result = projection
            .apply_events(&[ProjectionEvent::Upsert(path)])
            .await?;
        assert_eq!(result.committed.len(), 1);
        assert_eq!(
            records
                .load_note(&created.id)
                .await?
                .expect("resolved local creation")
                .visible
                .expect("visible local creation")
                .body,
            "new note\n"
        );
        node.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn synchronized_config_is_verified_and_materialized() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let node = IrohNode::persistent(directory.path().join("iroh")).await?;
        let workspace = node.create_workspace().await?;
        let records = WorkspaceRecords::new(&workspace);
        let source = b"(workspace-config \"{}\")\n".to_vec();
        records
            .put_config(
                "exo.scm",
                source.clone(),
                Hlc {
                    physical_ms: 4_000_000_000_000,
                    logical: 0,
                    actor_id: records.actor_id(),
                },
                BTreeSet::new(),
            )
            .await?;
        let index = LocalIndex::open(directory.path().join("index.sqlite"))?;
        let projection =
            WorkspaceProjection::open(&workspace, &index, directory.path().join("projection"))?;
        let report = projection.refresh().await?;
        assert_eq!(report.config_materialization.materialized.len(), 1);
        let path = projection.root().join("exo.scm");
        assert_eq!(std::fs::read(&path)?, source);
        let edited = b"(workspace-config \"{\\\"query_limit\\\":10}\")\n".to_vec();
        std::fs::write(&path, &edited)?;
        let applied = projection
            .apply_events(&[ProjectionEvent::Upsert(path)])
            .await?;
        assert_eq!(applied.committed.len(), 1);
        assert_eq!(records.list_configs().await?[0].bytes, edited);
        node.shutdown().await?;
        Ok(())
    }
}
