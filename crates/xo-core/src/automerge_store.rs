//! Automerge-backed storage for xo's canonical byte records.

use std::collections::BTreeMap;

use automerge::transaction::Transactable as _;
use automerge::{ActorId as AutomergeActorId, AutoCommit, ROOT, ReadDoc as _, ScalarValue};
use thiserror::Error;

const WORKSPACE_ID_KEY: &str = "$xo.workspace-id";
const SCHEMA_KEY: &str = "$xo.schema";
const RECORD_SCHEMA: u64 = 1;

#[derive(Debug, Error)]
pub enum AutomergeStoreError {
    #[error("Automerge operation failed: {0}")]
    Automerge(#[from] automerge::AutomergeError),
    #[error("workspace ID is missing from the Automerge document")]
    MissingWorkspaceId,
    #[error("Automerge value for {0:?} is not a byte record")]
    InvalidRecordValue(String),
    #[error("Automerge workspace metadata is invalid")]
    InvalidMetadata,
}

/// One Automerge document containing all small canonical records for a workspace.
#[derive(Clone, Debug)]
pub struct AutomergeRecordStore {
    document: AutoCommit,
    workspace_id: String,
}

impl AutomergeRecordStore {
    pub fn create(
        workspace_id: impl Into<String>,
        actor: &[u8],
    ) -> Result<Self, AutomergeStoreError> {
        let workspace_id = workspace_id.into();
        let mut document = AutoCommit::new().with_actor(AutomergeActorId::from(actor));
        document.put(ROOT, WORKSPACE_ID_KEY, workspace_id.clone())?;
        document.put(ROOT, SCHEMA_KEY, RECORD_SCHEMA)?;
        document.commit();
        Ok(Self {
            document,
            workspace_id,
        })
    }

    pub fn load(bytes: &[u8], actor: &[u8]) -> Result<Self, AutomergeStoreError> {
        let mut document = AutoCommit::load(bytes)?;
        document.set_actor(AutomergeActorId::from(actor));
        let workspace_id = document
            .get(ROOT, WORKSPACE_ID_KEY)?
            .and_then(|(value, _)| value.into_scalar().ok())
            .and_then(|value| value.into_string().ok())
            .ok_or(AutomergeStoreError::MissingWorkspaceId)?;
        let schema = document
            .get(ROOT, SCHEMA_KEY)?
            .and_then(|(value, _)| value.into_scalar().ok())
            .and_then(|value| value.to_u64());
        if schema != Some(RECORD_SCHEMA) {
            return Err(AutomergeStoreError::InvalidMetadata);
        }
        Ok(Self {
            document,
            workspace_id,
        })
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn put(&mut self, key: &str, value: Vec<u8>) -> Result<(), AutomergeStoreError> {
        self.document.put(ROOT, key, ScalarValue::Bytes(value))?;
        self.document.commit();
        Ok(())
    }

    pub fn transact<I>(&mut self, writes: I) -> Result<(), AutomergeStoreError>
    where
        I: IntoIterator<Item = (String, Vec<u8>)>,
    {
        for (key, value) in writes {
            self.document.put(ROOT, key, ScalarValue::Bytes(value))?;
        }
        self.document.commit();
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, AutomergeStoreError> {
        let Some((value, _)) = self.document.get(ROOT, key)? else {
            return Ok(None);
        };
        value
            .into_scalar()
            .ok()
            .and_then(|value| value.into_bytes().ok())
            .map(Some)
            .ok_or_else(|| AutomergeStoreError::InvalidRecordValue(key.to_owned()))
    }

    pub fn scan(&self, prefix: &str) -> Result<BTreeMap<String, Vec<u8>>, AutomergeStoreError> {
        self.document
            .keys(ROOT)
            .filter(|key| !key.starts_with("$xo.") && key.starts_with(prefix))
            .map(|key| {
                let value = self
                    .get(&key)?
                    .ok_or_else(|| AutomergeStoreError::InvalidRecordValue(key.clone()))?;
                Ok((key, value))
            })
            .collect()
    }

    #[must_use]
    pub fn heads(&mut self) -> Vec<String> {
        self.document
            .get_heads()
            .into_iter()
            .map(|head| head.to_string())
            .collect()
    }

    #[must_use]
    pub fn save(&mut self) -> Vec<u8> {
        self.document.save()
    }

    #[must_use]
    pub fn save_incremental(&mut self) -> Vec<u8> {
        self.document.save_incremental()
    }

    pub fn load_incremental(&mut self, bytes: &[u8]) -> Result<usize, AutomergeStoreError> {
        Ok(self.document.load_incremental(bytes)?)
    }

    pub fn merge(&mut self, other: &mut Self) -> Result<(), AutomergeStoreError> {
        self.document.merge(&mut other.document)?;
        Ok(())
    }

    pub fn apply_changes(
        &mut self,
        changes: impl IntoIterator<Item = automerge::Change>,
    ) -> Result<(), AutomergeStoreError> {
        self.document.apply_changes(changes)?;
        Ok(())
    }

    #[must_use]
    pub fn all_changes(&mut self) -> Vec<automerge::Change> {
        self.document
            .get_changes(&[])
            .into_iter()
            .cloned()
            .collect()
    }
}

/// Native durable wrapper. Every acknowledged mutation atomically replaces an fsynced snapshot.
#[cfg(feature = "native")]
#[derive(Debug)]
pub struct PersistentAutomergeStore {
    path: std::path::PathBuf,
    store: AutomergeRecordStore,
}

#[cfg(feature = "native")]
impl PersistentAutomergeStore {
    pub fn open_or_create(
        directory: &std::path::Path,
        workspace_id: &str,
        actor: &[u8],
    ) -> anyhow::Result<Self> {
        use anyhow::Context as _;

        std::fs::create_dir_all(directory)
            .with_context(|| format!("create {}", directory.display()))?;
        let path = directory.join("document.automerge");
        let store = match std::fs::read(&path) {
            Ok(bytes) => AutomergeRecordStore::load(&bytes, actor)
                .with_context(|| format!("load {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                AutomergeRecordStore::create(workspace_id, actor)?
            }
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        if store.workspace_id() != workspace_id {
            anyhow::bail!(
                "Automerge workspace {} does not match requested workspace {workspace_id}",
                store.workspace_id()
            );
        }
        let mut persistent = Self { path, store };
        persistent.flush()?;
        Ok(persistent)
    }

    #[must_use]
    pub fn store(&self) -> &AutomergeRecordStore {
        &self.store
    }

    pub fn put(&mut self, key: &str, value: Vec<u8>) -> anyhow::Result<()> {
        self.store.put(key, value)?;
        self.flush()
    }

    pub fn transact<I>(&mut self, writes: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = (String, Vec<u8>)>,
    {
        self.store.transact(writes)?;
        self.flush()
    }

    pub fn apply_incremental(&mut self, bytes: &[u8]) -> anyhow::Result<usize> {
        let changes = self.store.load_incremental(bytes)?;
        self.flush()?;
        Ok(changes)
    }

    pub fn merge_snapshot(&mut self, bytes: &[u8], actor: &[u8]) -> anyhow::Result<()> {
        let mut remote = AutomergeRecordStore::load(bytes, actor)?;
        if remote.workspace_id() != self.store.workspace_id() {
            anyhow::bail!("remote Automerge snapshot belongs to a different workspace");
        }
        self.store.merge(&mut remote)?;
        self.flush()
    }

    pub fn apply_changes(
        &mut self,
        changes: impl IntoIterator<Item = automerge::Change>,
    ) -> anyhow::Result<()> {
        self.store.apply_changes(changes)?;
        self.flush()
    }

    #[must_use]
    pub fn snapshot(&mut self) -> Vec<u8> {
        self.store.save()
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        let bytes = self.store.save();
        atomic_write(&self.path, &bytes)
    }
}

#[cfg(feature = "native")]
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Write as _;

    let temporary = path.with_extension("automerge.tmp");
    let mut file = std::fs::File::create(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("flush {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    #[cfg(unix)]
    {
        let parent = path.parent().context("Automerge snapshot has no parent")?;
        std::fs::File::open(parent)
            .with_context(|| format!("open {}", parent.display()))?
            .sync_all()
            .with_context(|| format!("flush {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_an_automerge_snapshot() {
        let mut store = AutomergeRecordStore::create("workspace-a", b"actor-a").unwrap();
        store.put("note/a/revision/1", vec![1, 2, 3]).unwrap();
        store.put("device/a", vec![4, 5]).unwrap();
        let bytes = store.save();

        let loaded = AutomergeRecordStore::load(&bytes, b"actor-a").unwrap();
        assert_eq!(loaded.workspace_id(), "workspace-a");
        assert_eq!(
            loaded.get("note/a/revision/1").unwrap(),
            Some(vec![1, 2, 3])
        );
        assert_eq!(loaded.scan("note/").unwrap().len(), 1);
    }

    #[test]
    fn concurrent_record_maps_converge() {
        let mut first = AutomergeRecordStore::create("workspace-a", b"actor-a").unwrap();
        let snapshot = first.save();
        let mut second = AutomergeRecordStore::load(&snapshot, b"actor-b").unwrap();
        first.put("note/a", vec![1]).unwrap();
        second.put("note/b", vec![2]).unwrap();

        first.merge(&mut second).unwrap();
        assert_eq!(first.get("note/a").unwrap(), Some(vec![1]));
        assert_eq!(first.get("note/b").unwrap(), Some(vec![2]));
    }

    #[test]
    fn transactions_commit_related_records_together() {
        let mut store = AutomergeRecordStore::create("workspace-a", b"actor-a").unwrap();
        store
            .transact([
                ("note/a/revision/1".to_owned(), vec![1]),
                ("note/a/head/a".to_owned(), vec![2]),
            ])
            .unwrap();
        assert_eq!(store.scan("note/a/").unwrap().len(), 2);
        assert_eq!(store.heads().len(), 1);
    }

    #[cfg(feature = "native")]
    #[test]
    fn durable_store_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        {
            let mut store = PersistentAutomergeStore::open_or_create(
                directory.path(),
                "workspace-a",
                b"actor-a",
            )
            .unwrap();
            store.put("note/a", vec![1, 2, 3]).unwrap();
        }
        let reopened =
            PersistentAutomergeStore::open_or_create(directory.path(), "workspace-a", b"actor-a")
                .unwrap();
        assert_eq!(reopened.store().get("note/a").unwrap(), Some(vec![1, 2, 3]));
    }
}
