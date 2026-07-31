//! Browser-facing WebAssembly facade for xo.
//!
//! This first slice proves that the sandboxed Steel runtime and a typed Wasm
//! boundary can be shipped as static PWA assets. Workspace and Iroh APIs will
//! be added behind the same worker-owned boundary.

use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use wasm_bindgen::prelude::*;

const MAX_PROBE_SOURCE_BYTES: usize = 64 * 1024;

/// Describe the browser runtime without exposing Rust implementation types.
#[wasm_bindgen]
#[must_use]
pub fn runtime_info() -> String {
    serde_json::json!({
        "api_version": 1,
        "crate_version": env!("CARGO_PKG_VERSION"),
        "steel": true,
        "iroh": false,
        "persistence": "indexeddb-shell",
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
        assert_eq!(info["steel"], true);
        assert_eq!(info["iroh"], false);
    }

    #[test]
    fn steel_probe_uses_a_fresh_sandbox() {
        assert_eq!(run_steel_inner("(+ 20 22)").unwrap(), "42");
        assert!(run_steel_inner("(get-env-var \"HOME\")").is_err());
    }
}
