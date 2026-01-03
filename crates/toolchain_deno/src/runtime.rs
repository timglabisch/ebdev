use deno_core::{JsRuntime, RuntimeOptions, PollEventLoopOptions, v8, ModuleSpecifier};
use std::{path::Path, rc::Rc};

use crate::module_loader::TsModuleLoader;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(e: std::io::Error) -> Self { Self(e.to_string()) } }
impl From<serde_json::Error> for Error { fn from(e: serde_json::Error) -> Self { Self(e.to_string()) } }

/// Load a TypeScript config file, returning the default export.
pub async fn load_ts_config<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut rt = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(TsModuleLoader(dir.to_path_buf()))),
        ..Default::default()
    });

    let module = ModuleSpecifier::from_file_path(&path).map_err(|_| Error("Invalid path".into()))?;

    // Load & evaluate
    let id = rt.load_main_es_module(&module).await.map_err(|e| Error(e.to_string()))?;
    let eval = rt.mod_evaluate(id);
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;
    eval.await.map_err(|e| Error(e.to_string()))?;

    // Extract default export
    let code = format!(r#"(async()=>{{globalThis.__r=JSON.stringify((await import("{}")).default)}})()"#, module);
    rt.execute_script("<x>", code).map_err(|e| Error(e.to_string()))?;
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;

    let result = rt.execute_script("<r>", "globalThis.__r").map_err(|e| Error(e.to_string()))?;
    let json = v8_string(&mut rt, result)?;
    serde_json::from_str(&json).map_err(Error::from)
}

fn v8_string(rt: &mut JsRuntime, val: v8::Global<v8::Value>) -> Result<String, Error> {
    let iso = rt.v8_isolate();
    let v = val.open(iso);
    if v.is_undefined() || v.is_null() {
        return Err(Error("Config file must have a default export (export default ...)".into()));
    }
    if !v.is_string() {
        return Err(Error("Default export must be an object (use defineConfig)".into()));
    }
    let s: &v8::String = unsafe { &*(v as *const v8::Value as *const v8::String) };
    Ok(s.to_rust_string_lossy(iso))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".ebdev.ts"), r#"
import { defineConfig, presets } from "ebdev";
export default defineConfig({
    toolchain: { node: "22.0.0" },
    mutagen: { sync: [{ ...presets.node, name: "app", target: "docker://x" }] },
});"#).unwrap();

        let r: serde_json::Value = load_ts_config(&dir.path().join(".ebdev.ts")).await.unwrap();
        assert_eq!(r["toolchain"]["node"]["version"], "22.0.0");
        assert!(r["mutagen"]["sync"][0]["ignore"].as_array().unwrap().contains(&serde_json::json!("node_modules")));
    }

    #[tokio::test]
    async fn test_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.ts"), r#"export const ver = "20.0.0";"#).unwrap();
        std::fs::write(dir.path().join(".ebdev.ts"), r#"
import { defineConfig } from "ebdev";
import { ver } from "./shared.ts";
export default defineConfig({ toolchain: { node: ver }, mutagen: { sync: [] } });"#).unwrap();

        let r: serde_json::Value = load_ts_config(&dir.path().join(".ebdev.ts")).await.unwrap();
        assert_eq!(r["toolchain"]["node"]["version"], "20.0.0");
    }
}
