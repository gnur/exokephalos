//! Browser-facing WebAssembly facade for xo.
//!
//! The dedicated browser worker owns this facade, its durable Automerge replica,
//! synchronization state, record resolution, and sandboxed Steel runtime.

mod browser_replica;
mod workspace;

use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use wasm_bindgen::prelude::*;

const MAX_PROBE_SOURCE_BYTES: usize = 64 * 1024;

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// Describe the browser runtime without exposing Rust implementation types.
#[wasm_bindgen]
#[must_use]
pub fn runtime_info() -> String {
    serde_json::json!({
        "api_version": 1,
        "version": env!("XO_BUILD_VERSION"),
        "steel": true,
        "central_sync": true,
        "persistence": "indexeddb-automerge-replica",
    })
    .to_string()
}

/// Execute Steel in a fresh sandboxed VM owned by the calling Web Worker.
///
/// The UI can terminate that worker if source does not complete. No filesystem,
/// process, socket, environment, or dynamic-library host functions are added.
#[wasm_bindgen]
pub fn run_steel(source: &str) -> Result<String, JsValue> {
    run_steel_inner(source).map_err(|error| JsValue::from_str(&error))
}

fn run_steel_inner(source: &str) -> Result<String, String> {
    if source.len() > MAX_PROBE_SOURCE_BYTES {
        return Err("Steel source exceeds 64 KiB".into());
    }
    let mut engine = Engine::new_sandboxed();
    let values = engine
        .run(source.to_owned())
        .map_err(|error| error.to_string())?;
    let value = values
        .last()
        .ok_or_else(|| "Steel source returned no value".to_owned())?;
    steel_value(value).ok_or_else(|| "Steel probe returned an unsupported value".to_owned())
}

/// Decode and resolve authoritative xo records into browser presentation data.
#[wasm_bindgen]
pub fn workspace_snapshot(entries_json: &str) -> Result<String, JsValue> {
    workspace::snapshot_json(entries_json).map_err(|error| JsValue::from_str(&format!("{error:#}")))
}

/// Execute a configured view/subview/search/tag query in authoritative Rust.
#[wasm_bindgen]
pub fn query_workspace(snapshot_json: &str, query_json: &str) -> Result<String, JsValue> {
    workspace::query_snapshot_json(snapshot_json, query_json)
        .map_err(|error| JsValue::from_str(&format!("{error:#}")))
}

/// Prepare immutable revision/head writes for create, edit, delete, or restore.
#[wasm_bindgen]
pub fn prepare_note_mutation(
    entries_json: &str,
    author: &str,
    input_json: &str,
    now_ms: u64,
    local_offset_seconds: i32,
) -> Result<String, JsValue> {
    workspace::prepare_mutation_json(
        entries_json,
        author,
        input_json,
        now_ms,
        local_offset_seconds,
    )
    .map_err(|error| JsValue::from_str(&format!("{error:#}")))
}

fn steel_value(value: &SteelVal) -> Option<String> {
    match value {
        SteelVal::BoolV(value) => Some(value.to_string()),
        SteelVal::IntV(value) => Some(value.to_string()),
        SteelVal::NumV(value) => Some(value.to_string()),
        SteelVal::StringV(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_contract_is_versioned() {
        let info: serde_json::Value = serde_json::from_str(&runtime_info()).unwrap();
        assert_eq!(info["api_version"], 1);
        assert_eq!(info["version"], env!("XO_BUILD_VERSION"));
        assert_eq!(info["steel"], true);
        assert_eq!(info["central_sync"], true);
    }

    #[test]
    fn steel_probe_uses_a_fresh_sandbox() {
        assert_eq!(run_steel_inner("(+ 20 22)").unwrap(), "42");
        assert!(run_steel_inner("(get-env-var \"HOME\")").is_err());
    }
}
