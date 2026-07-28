//! Verified offline backup and clean restore for persistent native peer state.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MANIFEST: &str = "manifest.json";
const PAYLOAD: &str = "payload";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub path: String,
    pub size: u64,
    pub blake3: String,
    pub mode: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema: u16,
    pub entries: Vec<BackupEntry>,
}

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("backup manifest error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("backup path is invalid: {0}")]
    InvalidPath(String),
    #[error("backup destination must not already exist: {0}")]
    DestinationExists(String),
    #[error("restore destination must be empty: {0}")]
    DestinationNotEmpty(String),
    #[error("backup verification failed for {0}")]
    Verification(String),
    #[error("atomic backup write failed: {0}")]
    Persist(#[from] tempfile::PersistError),
}

/// Create a verified directory backup. The peer must be shut down first.
pub fn create_backup(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<BackupManifest, BackupError> {
    let source = source.as_ref().canonicalize()?;
    let destination = absolute(destination.as_ref())?;
    if destination.exists() {
        return Err(BackupError::DestinationExists(
            destination.display().to_string(),
        ));
    }
    if destination.starts_with(&source) {
        return Err(BackupError::InvalidPath(destination.display().to_string()));
    }
    std::fs::create_dir_all(destination.join(PAYLOAD))?;
    let mut files = Vec::new();
    collect_files(&source, &source, &mut files)?;
    files.sort();
    let mut entries = Vec::new();
    for path in files {
        let relative = relative_string(&source, &path)?;
        let bytes = std::fs::read(&path)?;
        let output = destination.join(PAYLOAD).join(&relative);
        write_file(&output, &bytes, file_mode(&path)?)?;
        entries.push(BackupEntry {
            path: relative,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            blake3: blake3::hash(&bytes).to_hex().to_string(),
            mode: file_mode(&path)?,
        });
    }
    let manifest = BackupManifest { schema: 1, entries };
    write_file(
        &destination.join(MANIFEST),
        &serde_json::to_vec_pretty(&manifest)?,
        None,
    )?;
    verify_backup(&destination)?;
    Ok(manifest)
}

pub fn verify_backup(path: impl AsRef<Path>) -> Result<BackupManifest, BackupError> {
    let path = path.as_ref();
    let manifest: BackupManifest = serde_json::from_slice(&std::fs::read(path.join(MANIFEST))?)?;
    if manifest.schema != 1 {
        return Err(BackupError::Verification(
            "unsupported manifest schema".to_owned(),
        ));
    }
    for entry in &manifest.entries {
        validate_relative(&entry.path)?;
        let bytes = std::fs::read(path.join(PAYLOAD).join(&entry.path))?;
        if u64::try_from(bytes.len()).ok() != Some(entry.size)
            || blake3::hash(&bytes).to_hex().as_str() != entry.blake3
        {
            return Err(BackupError::Verification(entry.path.clone()));
        }
    }
    Ok(manifest)
}

/// Verify and restore a backup into a new or empty state directory.
pub fn restore_backup(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<BackupManifest, BackupError> {
    let backup = backup.as_ref();
    let manifest = verify_backup(backup)?;
    let destination = destination.as_ref();
    if destination.exists() && std::fs::read_dir(destination)?.next().is_some() {
        return Err(BackupError::DestinationNotEmpty(
            destination.display().to_string(),
        ));
    }
    std::fs::create_dir_all(destination)?;
    for entry in &manifest.entries {
        let bytes = std::fs::read(backup.join(PAYLOAD).join(&entry.path))?;
        write_file(&destination.join(&entry.path), &bytes, entry.mode)?;
    }
    Ok(manifest)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), BackupError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(BackupError::InvalidPath(path.display().to_string()));
        }
        if kind.is_dir() {
            collect_files(root, &path, files)?;
        } else if kind.is_file() {
            path.strip_prefix(root)
                .map_err(|_| BackupError::InvalidPath(path.display().to_string()))?;
            files.push(path);
        }
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), BackupError> {
    let parent = path
        .parent()
        .ok_or_else(|| BackupError::InvalidPath(path.display().to_string()))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    set_mode(temporary.path(), mode)?;
    temporary.persist(path)?;
    Ok(())
}

fn relative_string(root: &Path, path: &Path) -> Result<String, BackupError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BackupError::InvalidPath(path.display().to_string()))?;
    let value = relative.to_string_lossy().replace('\\', "/");
    validate_relative(&value)?;
    Ok(value)
}

fn validate_relative(value: &str) -> Result<(), BackupError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        Err(BackupError::InvalidPath(value.to_owned()))
    } else {
        Ok(())
    }
}

fn absolute(path: &Path) -> Result<PathBuf, BackupError> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<Option<u32>, std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    Ok(Some(std::fs::metadata(path)?.permissions().mode()))
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Result<Option<u32>, std::io::Error> {
    Ok(None)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "iroh-sync")]
    use std::collections::BTreeSet;
    #[cfg(feature = "iroh-sync")]
    use std::time::Duration;

    #[cfg(feature = "iroh-sync")]
    use crate::iroh_node::IrohNode;
    #[cfg(feature = "iroh-sync")]
    use crate::records::WorkspaceRecords;
    #[cfg(feature = "iroh-sync")]
    use crate::{ActorId, CURRENT_SCHEMA, DeviceRecord, Hlc, NoteId, NoteRevision, RevisionId};

    #[test]
    fn verified_backup_restores_exact_bytes_and_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("endpoint.key"), b"secret").unwrap();
        std::fs::write(source.join("nested/state"), b"state").unwrap();
        let backup = directory.path().join("backup");
        let manifest = create_backup(&source, &backup).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        let restored = directory.path().join("restored");
        restore_backup(&backup, &restored).unwrap();
        assert_eq!(
            std::fs::read(restored.join("nested/state")).unwrap(),
            b"state"
        );

        std::fs::write(backup.join("payload/nested/state"), b"corrupt").unwrap();
        assert!(matches!(
            verify_backup(backup),
            Err(BackupError::Verification(_))
        ));
    }

    #[cfg(feature = "iroh-sync")]
    async fn wait_for_asset(
        records: crate::records::WorkspaceRecords<'_>,
    ) -> anyhow::Result<Vec<u8>> {
        for _ in 0..300 {
            match records.get_asset(&crate::AssetId::new("image001")).await {
                Ok(Some(asset)) => return Ok(asset.bytes),
                Ok(None)
                | Err(
                    crate::records::RecordError::MissingBlob(_)
                    | crate::records::RecordError::Transport(_),
                ) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("asset did not become available")
    }

    #[cfg(feature = "iroh-sync")]
    async fn wait_for_device(
        records: WorkspaceRecords<'_>,
        author: &ActorId,
    ) -> anyhow::Result<DeviceRecord> {
        for _ in 0..100 {
            match records.list_devices().await {
                Ok(devices) => {
                    if let Some(device) = devices
                        .into_iter()
                        .find(|device| &device.author_id == author)
                    {
                        return Ok(device);
                    }
                }
                Err(crate::records::RecordError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("device did not become available")
    }

    #[cfg(feature = "iroh-sync")]
    fn revision(
        actor: ActorId,
        body: &str,
        physical_ms: u64,
        predecessor: Option<RevisionId>,
    ) -> NoteRevision {
        NoteRevision {
            schema: CURRENT_SCHEMA,
            note_id: NoteId::new("note002"),
            frontmatter: crate::domain::Frontmatter::new(),
            body: body.to_owned(),
            materialized_path: "notes/restored.md".to_owned(),
            hlc: Hlc {
                physical_ms,
                logical: 0,
                actor_id: actor.clone(),
            },
            author_id: actor,
            predecessors: predecessor.into_iter().collect(),
            deleted: false,
        }
    }

    #[cfg(feature = "iroh-sync")]
    async fn wait_for_raw_record(
        workspace: &crate::iroh_node::IrohWorkspace,
        key: &str,
    ) -> anyhow::Result<()> {
        for _ in 0..100 {
            match workspace.get(key).await {
                Ok(Some(_)) => return Ok(()),
                Ok(None) | Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        anyhow::bail!("raw record did not replicate")
    }

    #[cfg(feature = "iroh-sync")]
    #[tokio::test]
    async fn restored_peer_serves_blobs_and_rejoins_an_active_peer() -> anyhow::Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;

        let directory = tempfile::tempdir()?;
        let active_dir = directory.path().join("active");
        let active = IrohNode::persistent(&active_dir).await?;
        let workspace = active.create_workspace().await?;
        let workspace_id = workspace.id();
        let active_records = WorkspaceRecords::new(&workspace);
        let active_author = active_records.actor_id();
        active_records
            .put_device(&DeviceRecord {
                schema: CURRENT_SCHEMA,
                endpoint_id: active.endpoint_id().to_string(),
                author_id: active_author.clone(),
                label: "active".to_owned(),
                capabilities: BTreeSet::from(["write".to_owned()]),
                last_seen_ms: Some(100),
                retired_at: None,
            })
            .await?;
        let before_id = active_records
            .commit_revision(&revision(active_author.clone(), "accepted", 100, None))
            .await?;
        active_records
            .put_asset(
                crate::AssetId::new("image001"),
                "image/png",
                "assets/example.png",
                b"backed up blob".to_vec(),
            )
            .await?;
        let ticket = workspace.share(true).await?;
        let replica_dir = directory.path().join("replica");
        let replica = IrohNode::persistent(&replica_dir).await?;
        let imported = replica.import_workspace(&ticket).await?;
        let replica_records = WorkspaceRecords::new(&imported);
        assert_eq!(wait_for_asset(replica_records).await?, b"backed up blob");
        let mut active_device = wait_for_device(replica_records, &active_author).await?;
        active.shutdown().await?;
        drop((workspace, active));
        active_device.retired_at = Some(Hlc {
            physical_ms: 200,
            logical: 0,
            actor_id: replica_records.actor_id(),
        });
        replica_records.put_device(&active_device).await?;
        replica.shutdown().await?;
        drop((imported, replica));

        let backup = directory.path().join("backup");
        create_backup(&replica_dir, &backup)?;
        let restored_dir = directory.path().join("restored");
        restore_backup(&backup, &restored_dir)?;
        let restored = IrohNode::persistent(&restored_dir).await?;
        let restored_workspace = restored
            .open_workspace(workspace_id)
            .await?
            .expect("restored workspace");
        assert_eq!(
            wait_for_asset(WorkspaceRecords::new(&restored_workspace)).await?,
            b"backed up blob"
        );

        let active = IrohNode::persistent(&active_dir).await?;
        let workspace = active
            .open_workspace(workspace_id)
            .await?
            .expect("active workspace");
        let after_id = WorkspaceRecords::new(&workspace)
            .commit_revision(&revision(
                active_author,
                "rejected after retirement",
                300,
                Some(before_id.clone()),
            ))
            .await?;
        workspace.put("health/rejoin", "connected").await?;
        restored_workspace
            .start_sync(&workspace.share(true).await?)
            .await?;
        let mut rejoined = false;
        for _ in 0..200 {
            if let Ok(Some(_)) = restored_workspace.get("health/rejoin").await {
                rejoined = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(rejoined);
        wait_for_raw_record(
            &restored_workspace,
            &format!("note/note002/revision/{after_id}"),
        )
        .await?;
        let resolved = WorkspaceRecords::new(&restored_workspace)
            .load_note(&NoteId::new("note002"))
            .await?
            .expect("retained pre-retirement note");
        assert_eq!(resolved.winning_revision, before_id);
        assert_eq!(resolved.visible.expect("visible note").body, "accepted");
        restored.shutdown().await?;
        active.shutdown().await?;
        Ok(())
    }
}
