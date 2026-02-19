use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions};
use ::ebdev_task_runner::TaskRunnerHandle;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::module_loader::TsModuleLoader;
use std::collections::HashMap;
use crate::ops::{ebdev_deno_ops, init_bridge_state, init_mutagen_state, init_task_runner_state};
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
        init_task_runner_state(&mut state, None, None, HashMap::new());
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
    env: HashMap<String, String>,
    embedded_linux_binary: &'static [u8],
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
        init_task_runner_state(&mut state, handle, Some(dir.to_string_lossy().to_string()), env);
        init_mutagen_state(&mut state, mutagen_path, path.clone());
        init_bridge_state(&mut state, embedded_linux_binary);
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
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "build", Some(handle), None, HashMap::new(), b"").await;

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
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "nonexistent", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_fs_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_file = dir.path().join("test-output.txt");
        let test_file_str = test_file.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_fs() {{
    await fs.writeFile("{test_file_str}", "hello from fs API\nline2");
    const content = await fs.readFile("{test_file_str}");
    if (content !== "hello from fs API\nline2") {{
        throw new Error("Content mismatch: " + JSON.stringify(content));
    }}
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_fs", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_fs should succeed: {:?}", result);

        // Also verify the file was actually written
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "hello from fs API\nline2");
    }

    #[tokio::test]
    async fn test_fs_append() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_file = dir.path().join("test-append.txt");
        let test_file_str = test_file.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_append() {{
    await fs.writeFile("{test_file_str}", "line1\n");
    await fs.appendFile("{test_file_str}", "line2\n");
    await fs.appendFile("{test_file_str}", "line3\n");
    const content = await fs.readFile("{test_file_str}");
    if (content !== "line1\nline2\nline3\n") {{
        throw new Error("Content mismatch: " + JSON.stringify(content));
    }}
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_append", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_append should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_mkdir_and_stat() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_dir = dir.path().join("nested/deep/dir");
        let test_dir_str = test_dir.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_mkdir() {{
    await fs.mkdir("{test_dir_str}");
    const stat = await fs.stat("{test_dir_str}");
    if (!stat.exists) throw new Error("dir should exist");
    if (!stat.isDir) throw new Error("should be a directory");
    if (stat.isFile) throw new Error("should not be a file");
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_mkdir", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_mkdir should succeed: {:?}", result);
        assert!(test_dir.exists(), "Directory should have been created");
    }

    #[tokio::test]
    async fn test_fs_exists_and_rm() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_file = dir.path().join("to-delete.txt");
        let test_file_str = test_file.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_rm() {{
    await fs.writeFile("{test_file_str}", "delete me");
    let exists = await fs.exists("{test_file_str}");
    if (!exists) throw new Error("file should exist after write");

    await fs.rm("{test_file_str}");
    exists = await fs.exists("{test_file_str}");
    if (exists) throw new Error("file should not exist after rm");
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_rm", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_rm should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_rm_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_dir = dir.path().join("tree-to-delete");
        let test_dir_str = test_dir.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_rm_recursive() {{
    await fs.mkdir("{test_dir_str}/sub/deep");
    await fs.writeFile("{test_dir_str}/sub/deep/file.txt", "content");
    await fs.writeFile("{test_dir_str}/root.txt", "root content");

    await fs.rm("{test_dir_str}", {{ recursive: true }});
    const exists = await fs.exists("{test_dir_str}");
    if (exists) throw new Error("dir should not exist after recursive rm");
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_rm_recursive", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_rm_recursive should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_stat_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, fs } from "ebdev";

export default defineConfig({});

export async function test_stat_missing() {
    const stat = await fs.stat("/tmp/this-file-does-not-exist-98765.txt");
    if (stat.exists) throw new Error("should not exist");
    if (stat.isFile) throw new Error("should not be a file");
    if (stat.isDir) throw new Error("should not be a dir");
    if (stat.size !== 0) throw new Error("size should be 0");
}
"#).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_stat_missing", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_stat_missing should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_read_nonexistent_throws() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, fs } from "ebdev";

export default defineConfig({});

export async function test_read_error() {
    try {
        await fs.readFile("/tmp/this-file-does-not-exist-98765.txt");
        throw new Error("Should have thrown");
    } catch (e) {
        if (!e.message.includes("fs.readFile")) {
            throw new Error("Error should mention fs.readFile: " + e.message);
        }
    }
}
"#).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_read_error", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_read_error should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_write_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, fs } from "ebdev";

export default defineConfig({});

export async function test_write_error() {
    try {
        await fs.writeFile("/nonexistent-root-dir/file.txt", "data");
        throw new Error("Should have thrown");
    } catch (e) {
        if (!e.message.includes("fs.writeFile")) {
            throw new Error("Error should mention fs.writeFile: " + e.message);
        }
    }
}
"#).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_write_error", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_write_error should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_fs_stat_file_size() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let test_file = dir.path().join("sized.txt");
        let test_file_str = test_file.to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, fs }} from "ebdev";

export default defineConfig({{}});

export async function test_stat_size() {{
    await fs.writeFile("{test_file_str}", "12345");
    const stat = await fs.stat("{test_file_str}");
    if (!stat.exists) throw new Error("should exist");
    if (!stat.isFile) throw new Error("should be file");
    if (stat.size !== 5) throw new Error("size should be 5, got " + stat.size);
}}
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(&config_path, "test_stat_size", Some(handle), None, HashMap::new(), b"").await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_stat_size should succeed: {:?}", result);
    }
}
