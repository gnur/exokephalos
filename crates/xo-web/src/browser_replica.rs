use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use js_sys::Uint8Array;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use xo_core::automerge_store::AutomergeRecordStore;
use xo_core::record_workspace::record_author;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentEntry {
    key: String,
    key_base64: String,
    value: Option<String>,
    value_base64: String,
    author: String,
    content_hash: String,
    content_len: u64,
}

#[wasm_bindgen]
pub struct BrowserReplica {
    store: AutomergeRecordStore,
    sync: automerge::sync::State,
    actor: String,
}

#[wasm_bindgen]
impl BrowserReplica {
    #[wasm_bindgen(js_name = create)]
    pub fn create(workspace_id: &str, actor: String) -> Result<Self, JsError> {
        validate_actor(&actor)?;
        Ok(Self {
            store: AutomergeRecordStore::create(workspace_id, actor.as_bytes())?,
            sync: automerge::sync::State::new(),
            actor,
        })
    }

    #[wasm_bindgen(js_name = restore)]
    pub fn restore(snapshot: &Uint8Array, actor: String) -> Result<Self, JsError> {
        validate_actor(&actor)?;
        Ok(Self {
            store: AutomergeRecordStore::load(&snapshot.to_vec(), actor.as_bytes())?,
            sync: automerge::sync::State::new(),
            actor,
        })
    }

    #[wasm_bindgen(js_name = workspaceId)]
    #[must_use]
    pub fn workspace_id(&self) -> String {
        self.store.workspace_id().to_owned()
    }

    #[wasm_bindgen(js_name = resetSync)]
    pub fn reset_sync(&mut self) {
        self.sync = automerge::sync::State::new();
    }

    #[wasm_bindgen(js_name = put)]
    pub fn put(&mut self, key: &str, value: &Uint8Array) -> Result<(), JsError> {
        if key.is_empty() || key.len() > 1024 {
            return Err(JsError::new("record key must contain 1–1024 bytes"));
        }
        self.store.put(key, value.to_vec())?;
        Ok(())
    }

    #[wasm_bindgen(js_name = snapshot)]
    pub fn snapshot(&mut self) -> Uint8Array {
        Uint8Array::from(self.store.save().as_slice())
    }

    #[wasm_bindgen(js_name = entriesJson)]
    pub fn entries_json(&self) -> Result<String, JsError> {
        let entries = self
            .store
            .scan("")?
            .into_iter()
            .map(|(key, value)| {
                let key_base64 = BASE64.encode(key.as_bytes());
                let value_base64 = BASE64.encode(&value);
                let author =
                    record_author(key.as_bytes(), &value).unwrap_or_else(|| self.actor.clone());
                let content_hash = blake3::hash(&value).to_hex().to_string();
                let content_len = u64::try_from(value.len()).unwrap_or(u64::MAX);
                let value_text = String::from_utf8(value).ok();
                DocumentEntry {
                    key,
                    key_base64,
                    value: value_text,
                    value_base64,
                    author,
                    content_hash,
                    content_len,
                }
            })
            .collect::<Vec<_>>();
        Ok(serde_json::to_string(&entries)?)
    }

    #[wasm_bindgen(js_name = generateSyncMessage)]
    pub fn generate_sync_message(&mut self) -> Option<Uint8Array> {
        self.store
            .generate_sync_message(&mut self.sync)
            .map(|bytes| Uint8Array::from(bytes.as_slice()))
    }

    #[wasm_bindgen(js_name = receiveSyncMessage)]
    pub fn receive_sync_message(&mut self, message: &Uint8Array) -> Result<bool, JsError> {
        self.store
            .receive_sync_message(&mut self.sync, &message.to_vec())
            .map_err(|error| JsError::new(&format!("{error:#}")))
    }
}

fn validate_actor(actor: &str) -> Result<(), JsError> {
    if actor.is_empty() || actor.len() > 128 {
        Err(JsError::new("client ID must contain 1–128 bytes"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_snapshot_restores_records() {
        let mut first = BrowserReplica::create("workspace-test", "browser-a".into()).unwrap();
        first.store.put("test/value", b"durable".to_vec()).unwrap();
        let snapshot = first.store.save();
        let restored = AutomergeRecordStore::load(&snapshot, b"browser-a").unwrap();
        assert_eq!(
            restored.get("test/value").unwrap().as_deref(),
            Some(b"durable".as_slice())
        );
    }
}
