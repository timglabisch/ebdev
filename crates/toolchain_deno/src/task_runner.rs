use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions};
use ::ebdev_task_runner::TaskRunnerHandle;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::module_loader::TsModuleLoader;
use crate::ops::{ebdev_deno_ops, init_mutagen_state, init_task_runner_state};
use crate::runtime::Error;

/// List all exported async functions (tasks) from a .ebdev.ts file
pub async fn list_tasks(path: &Path) -> Result<Vec<String>, Error> {
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut rt = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(TsModuleLoader(dir.to_path_buf()))),
        extensions: vec![ebdev_deno_ops::init()],
        ..Default::default()
    });

    // Initialize task runner state (no event sender for list)
    {
        let op_state = rt.op_state();
        let mut state = op_state.borrow_mut();
        init_task_runner_state(&mut state, None, None);
    }

    let module = ModuleSpecifier::from_file_path(&path).map_err(|_| Error("Invalid path".into()))?;

    // Load module
    let id = rt.load_main_es_module(&module).await.map_err(|e| Error(e.to_string()))?;
    let eval = rt.mod_evaluate(id);
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;
    eval.await.map_err(|e| Error(e.to_string()))?;

    // Get all exported function names
    let code = format!(r#"
        (async () => {{
            const mod = await import("{}");
            const tasks = [];
            for (const [name, value] of Object.entries(mod)) {{
                if (typeof value === 'function' && name !== 'default') {{
                    tasks.push(name);
                }}
            }}
            globalThis.__tasks = JSON.stringify(tasks);
        }})()
    "#, module);

    rt.execute_script("<list>", code).map_err(|e| Error(e.to_string()))?;
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;

    let result = rt.execute_script("<r>", "globalThis.__tasks").map_err(|e| Error(e.to_string()))?;
    let json = v8_string(&mut rt, result)?;
    serde_json::from_str(&json).map_err(|e| Error(e.to_string()))
}

/// Run a specific task from a .ebdev.ts file
pub async fn run_task(
    path: &Path,
    task_name: &str,
    handle: Option<TaskRunnerHandle>,
    mutagen_path: Option<PathBuf>,
) -> Result<(), Error> {
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut rt = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(TsModuleLoader(dir.to_path_buf()))),
        extensions: vec![ebdev_deno_ops::init()],
        ..Default::default()
    });

    // Initialize task runner state
    {
        let op_state = rt.op_state();
        let mut state = op_state.borrow_mut();
        init_task_runner_state(&mut state, handle, Some(dir.to_string_lossy().to_string()));
        init_mutagen_state(&mut state, mutagen_path, path.clone());
    }

    let module = ModuleSpecifier::from_file_path(&path).map_err(|_| Error("Invalid path".into()))?;

    // Load module
    let id = rt.load_main_es_module(&module).await.map_err(|e| Error(e.to_string()))?;
    let eval = rt.mod_evaluate(id);
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;
    eval.await.map_err(|e| Error(e.to_string()))?;

    // Call the task function
    let code = format!(r#"
        (async () => {{
            const mod = await import("{}");
            const task = mod["{}"];
            if (!task) {{
                throw new Error("Task '{}' not found. Available tasks: " +
                    Object.keys(mod).filter(k => typeof mod[k] === 'function' && k !== 'default').join(', '));
            }}
            if (typeof task !== 'function') {{
                throw new Error("'{}' is not a function");
            }}
            await task();
            globalThis.__taskResult = "ok";
        }})()
    "#, module, task_name, task_name, task_name);

    rt.execute_script("<run>", code).map_err(|e| Error(e.to_string()))?;
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;

    // Check result
    let result = rt.execute_script("<r>", "globalThis.__taskResult").map_err(|e| Error(e.to_string()))?;
    let result_str = v8_string(&mut rt, result)?;

    if result_str != "ok" {
        return Err(Error(format!("Task failed: {}", result_str)));
    }

    Ok(())
}

fn v8_string(rt: &mut JsRuntime, val: deno_core::v8::Global<deno_core::v8::Value>) -> Result<String, Error> {
    let iso = rt.v8_isolate();
    let v = val.open(iso);
    if v.is_undefined() || v.is_null() {
        return Err(Error("Result is undefined".into()));
    }
    if !v.is_string() {
        return Err(Error("Result is not a string".into()));
    }
    let s: &deno_core::v8::String = unsafe { &*(v as *const deno_core::v8::Value as *const deno_core::v8::String) };
    Ok(s.to_rust_string_lossy(iso))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig } from "ebdev";

export default defineConfig({});

export async function build() {
    console.log("building...");
}

export async function test() {
    console.log("testing...");
}

export async function deploy() {
    console.log("deploying...");
}

// Not a task (not a function)
export const VERSION = "1.0.0";
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        assert!(tasks.contains(&"build".to_string()));
        assert!(tasks.contains(&"test".to_string()));
        assert!(tasks.contains(&"deploy".to_string()));
        assert!(!tasks.contains(&"VERSION".to_string()));
        assert!(!tasks.contains(&"default".to_string()));
    }

    #[tokio::test]
    async fn test_run_task_simple() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, exec }} from "ebdev";

export default defineConfig({{}});

export async function build() {{
    await exec(["echo", "hello from task"]);
}}
"#)).unwrap();

        // Run with headless task runner
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None);
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "build", Some(handle), None).await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "Task should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_run_task_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig } from "ebdev";

export default defineConfig({});

export async function build() {}
"#).unwrap();

        // Run with headless task runner
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None);
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "nonexistent", Some(handle), None).await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
