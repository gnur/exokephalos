use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const FILE_NAME: &str = "api-keys.json";
const PREFIX: &str = "xok_";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiKeyMetadata {
    pub id: String,
    pub label: String,
    pub permissions: BTreeSet<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredKey {
    #[serde(flatten)]
    metadata: ApiKeyMetadata,
    owner: String,
    hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CreatedApiKey {
    #[serde(flatten)]
    pub metadata: ApiKeyMetadata,
    pub token: String,
}

#[derive(Debug)]
pub struct ApiKeys {
    path: PathBuf,
    keys: Mutex<Vec<StoredKey>>,
}

impl ApiKeys {
    pub fn open(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join(FILE_NAME);
        let keys = if path.exists() {
            serde_json::from_slice(
                &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
            )
            .context("decode API key store")?
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            keys: Mutex::new(keys),
        })
    }

    pub fn authorize(&self, token: &str, permission: &str) -> bool {
        let hash = digest(token);
        self.keys.lock().ok().is_some_and(|keys| {
            keys.iter()
                .any(|key| key.hash == hash && key.metadata.permissions.contains(permission))
        })
    }

    pub fn list(&self, owner: &str) -> Vec<ApiKeyMetadata> {
        self.keys.lock().map_or_else(
            |_| Vec::new(),
            |keys| {
                keys.iter()
                    .filter(|key| key.owner == owner)
                    .map(|key| key.metadata.clone())
                    .collect()
            },
        )
    }

    pub fn create(
        &self,
        owner: &str,
        label: &str,
        permissions: BTreeSet<String>,
    ) -> Result<CreatedApiKey> {
        if label.trim().is_empty() || label.len() > 120 {
            bail!("API key label must contain 1 to 120 characters");
        }
        if permissions.is_empty() {
            bail!("API key must grant at least one permission");
        }
        let mut random = [0_u8; 32];
        rand::rng().fill_bytes(&mut random);
        let token = format!("{PREFIX}{}", hex(&random));
        let metadata = ApiKeyMetadata {
            id: hex(&random[..8]),
            label: label.trim().to_owned(),
            permissions,
            created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        };
        let mut keys = self
            .keys
            .lock()
            .map_err(|_| anyhow::anyhow!("API key store lock poisoned"))?;
        keys.push(StoredKey {
            metadata: metadata.clone(),
            owner: owner.to_owned(),
            hash: digest(&token),
        });
        self.save(&keys)?;
        Ok(CreatedApiKey { metadata, token })
    }

    pub fn remove(&self, owner: &str, id: &str) -> Result<bool> {
        let mut keys = self
            .keys
            .lock()
            .map_err(|_| anyhow::anyhow!("API key store lock poisoned"))?;
        let before = keys.len();
        keys.retain(|key| key.metadata.id != id || key.owner != owner);
        if keys.len() == before {
            return Ok(false);
        }
        self.save(&keys)?;
        Ok(true)
    }

    fn save(&self, keys: &[StoredKey]) -> Result<()> {
        let temporary = self.path.with_extension("json.new");
        let encoded = serde_json::to_vec_pretty(keys)?;
        std::fs::write(&temporary, encoded)
            .with_context(|| format!("write {}", temporary.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temporary, &self.path)
            .with_context(|| format!("replace {}", self.path.display()))
    }
}

fn digest(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("write hex to String");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_hashed_scoped_owned_and_durable() {
        let directory = tempfile::tempdir().unwrap();
        let keys = ApiKeys::open(directory.path()).unwrap();
        let created = keys
            .create(
                "alice",
                "automation",
                BTreeSet::from(["xo:read".into(), "xo:write".into()]),
            )
            .unwrap();
        assert!(created.token.starts_with(PREFIX));
        assert!(keys.authorize(&created.token, "xo:read"));
        assert!(!keys.authorize(&created.token, "xo:sync"));
        assert_eq!(keys.list("alice").len(), 1);
        assert!(keys.list("bob").is_empty());
        assert!(
            !std::fs::read_to_string(directory.path().join(FILE_NAME))
                .unwrap()
                .contains(&created.token)
        );
        drop(keys);

        let reopened = ApiKeys::open(directory.path()).unwrap();
        assert!(reopened.authorize(&created.token, "xo:write"));
        assert!(reopened.remove("alice", &created.metadata.id).unwrap());
        assert!(!reopened.authorize(&created.token, "xo:read"));
    }
}
