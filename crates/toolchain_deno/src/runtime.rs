use deno_core::{JsRuntime, RuntimeOptions, PollEventLoopOptions, v8, ModuleSpecifier};
use std::path::Path;
use std::rc::Rc;
use thiserror::Error;

use crate::module_loader::TsModuleLoader;

#[derive(Debug, Error)]
pub enum TsRuntimeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JavaScript error: {0}")]
    JsError(String),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Module error: {0}")]
    ModuleError(String),
}

/// Extract a JSON string from script result
fn v8_to_string(runtime: &mut JsRuntime, value: v8::Global<v8::Value>) -> Result<String, TsRuntimeError> {
    let isolate = runtime.v8_isolate();
    let value = value.open(isolate);

    if !value.is_string() {
        return Err(TsRuntimeError::JsError("Expected string".into()));
    }
    // SAFETY: is_string() verified above, pointer cast is valid for v8 Value hierarchy
    let s: &v8::String = unsafe { &*(value as *const v8::Value as *const v8::String) };
    Ok(s.to_rust_string_lossy(isolate))
}

/// Load and execute a TypeScript config file as an ES module, returning the default export.
///
/// Supports `import { foo } from "./other.ts"` and `import { defineConfig } from "ebdev"`.
pub async fn load_ts_config<T: serde::de::DeserializeOwned>(config_path: &Path) -> Result<T, TsRuntimeError> {
    let config_path = config_path.canonicalize()?;
    let config_dir = config_path.parent().unwrap_or(Path::new("."));

    let module_loader = TsModuleLoader::new(config_dir.to_path_buf());
    let mut runtime = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(module_loader)),
        ..Default::default()
    });

    let main_module = ModuleSpecifier::from_file_path(&config_path)
        .map_err(|_| TsRuntimeError::ModuleError("Invalid config path".into()))?;

    // Load and evaluate module
    let mod_id = runtime.load_main_es_module(&main_module).await
        .map_err(|e| TsRuntimeError::ModuleError(e.to_string()))?;

    let eval_result = runtime.mod_evaluate(mod_id);
    runtime.run_event_loop(PollEventLoopOptions::default()).await
        .map_err(|e| TsRuntimeError::JsError(e.to_string()))?;
    eval_result.await
        .map_err(|e| TsRuntimeError::JsError(e.to_string()))?;

    // Extract default export via dynamic import (module is cached)
    let code = format!(
        r#"(async () => {{ globalThis.__r = JSON.stringify((await import("{}")).default); }})()"#,
        main_module.as_str()
    );
    runtime.execute_script("<extract>", code)
        .map_err(|e| TsRuntimeError::JsError(e.to_string()))?;

    runtime.run_event_loop(PollEventLoopOptions::default()).await
        .map_err(|e| TsRuntimeError::JsError(e.to_string()))?;

    let result = runtime.execute_script("<result>", "globalThis.__r".to_string())
        .map_err(|e| TsRuntimeError::JsError(e.to_string()))?;

    let json_str = v8_to_string(&mut runtime, result)?;
    serde_json::from_str(&json_str).map_err(TsRuntimeError::JsonError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".ebdev.ts"), r#"
import { defineConfig, presets } from "ebdev";
export default defineConfig({
    toolchain: { node: "22.0.0" },
    mutagen: { sync: [{ ...presets.node, name: "app", target: "docker://x" }] },
});
"#).unwrap();

        let result: serde_json::Value = load_ts_config(&dir.path().join(".ebdev.ts")).await.unwrap();
        assert_eq!(result["toolchain"]["node"]["version"], "22.0.0");
        assert_eq!(result["mutagen"]["sync"][0]["name"], "app");
        // Also tests presets work
        assert!(result["mutagen"]["sync"][0]["ignore"].as_array().unwrap().contains(&serde_json::json!("node_modules")));
    }

    #[tokio::test]
    async fn test_local_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.ts"), r#"export const ver = "20.0.0";"#).unwrap();
        std::fs::write(dir.path().join(".ebdev.ts"), r#"
import { defineConfig } from "ebdev";
import { ver } from "./shared.ts";
export default defineConfig({ toolchain: { node: ver }, mutagen: { sync: [] } });
"#).unwrap();

        let result: serde_json::Value = load_ts_config(&dir.path().join(".ebdev.ts")).await.unwrap();
        assert_eq!(result["toolchain"]["node"]["version"], "20.0.0");
    }
}
