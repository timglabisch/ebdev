use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions};
use ::ebdev_task_runner::TaskRunnerHandle;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::module_loader::TsModuleLoader;
use std::collections::HashMap;
use crate::ops::{ebdev_deno_ops, init_bridge_state, init_mutagen_state, init_task_runner_state};
use crate::runtime::Error;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<ArgInfo>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArgInfo {
    pub name: String,
    #[serde(rename = "cliName")]
    pub cli_name: String,
    #[serde(rename = "type")]
    pub arg_type: String,
    pub description: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub completable: bool,
}

/// Create a JsRuntime, load the given module, and return both ready for script execution.
async fn create_runtime(path: &Path, init_state: impl FnOnce(&mut deno_core::OpState)) -> Result<(JsRuntime, ModuleSpecifier), Error> {
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut rt = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(TsModuleLoader(dir.to_path_buf()))),
        extensions: vec![ebdev_deno_ops::init()],
        ..Default::default()
    });

    {
        let op_state = rt.op_state();
        let mut state = op_state.borrow_mut();
        init_state(&mut state);
    }

    let module = ModuleSpecifier::from_file_path(path).map_err(|_| Error("Invalid path".into()))?;

    let id = rt.load_main_es_module(&module).await.map_err(|e| Error(e.to_string()))?;
    let eval = rt.mod_evaluate(id);
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;
    eval.await.map_err(|e| Error(e.to_string()))?;

    Ok((rt, module))
}

/// Execute a JS script, run the event loop, then read a global string result.
async fn exec_and_read_global(rt: &mut JsRuntime, label: &'static str, code: String, global: &'static str) -> Result<String, Error> {
    rt.execute_script(label, code).map_err(|e| Error(e.to_string()))?;
    rt.run_event_loop(PollEventLoopOptions::default()).await.map_err(|e| Error(e.to_string()))?;

    let result = rt.execute_script("<r>", global.to_string()).map_err(|e| Error(e.to_string()))?;
    v8_string(rt, result)
}

/// List all exported async functions (tasks) from a .ebdev.ts file
pub async fn list_tasks(path: &Path) -> Result<Vec<TaskInfo>, Error> {
    let path = path.canonicalize()?;

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, None, None, HashMap::new());
    }).await?;

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const tasks = [];
            for (const [name, value] of Object.entries(mod)) {{
                if (typeof value === 'function' && name !== 'default') {{
                    const info = {{ name }};
                    if (value.__ebdevTaskDef) {{
                        const def = value.__ebdevTaskDef;
                        if (def.description) info.description = def.description;
                        const schema = def.__argSchema;
                        if (schema && Object.keys(schema).length > 0) {{
                            info.args = [];
                            for (const [argName, builder] of Object.entries(schema)) {{
                                const cliName = argName.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
                                const argInfo = {{
                                    name: argName,
                                    cliName,
                                    type: builder._type,
                                    description: builder._description || "",
                                    required: !!builder._required,
                                }};
                                if (builder._default !== undefined) argInfo.default = builder._default;
                                if (builder._choices) argInfo.choices = builder._choices;
                                if (builder._completeFn) argInfo.completable = true;
                                info.args.push(argInfo);
                            }}
                        }}
                    }}
                    tasks.push(info);
                }}
            }}
            globalThis.__tasks = JSON.stringify(tasks);
        }})()
    "#);

    let json = exec_and_read_global(&mut rt, "<list>", code, "globalThis.__tasks").await?;
    serde_json::from_str(&json).map_err(|e| Error(e.to_string()))
}

/// Run the completion function for a specific arg of a specific task.
/// Returns the list of completion values, or an empty vec on error.
pub async fn complete_arg(path: &Path, task_name: &str, arg_name: &str) -> Result<Vec<String>, Error> {
    let path = path.canonicalize()?;

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, None, None, HashMap::new());
    }).await?;

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const task = mod["{task_name}"];
            if (!task || !task.__ebdevTaskDef) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            const schema = task.__ebdevTaskDef.__argSchema;
            if (!schema) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            const builder = schema["{arg_name}"];
            if (!builder || !builder._completeFn) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            try {{
                const values = await builder._completeFn();
                globalThis.__completeResult = JSON.stringify(Array.isArray(values) ? values : []);
            }} catch (e) {{
                globalThis.__completeResult = "[]";
            }}
        }})()
    "#);

    let json = exec_and_read_global(&mut rt, "<complete>", code, "globalThis.__completeResult").await?;
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
    task_args: Vec<String>,
) -> Result<(), Error> {
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, handle, Some(dir.to_string_lossy().to_string()), env);
        init_mutagen_state(state, mutagen_path, path.clone());
        init_bridge_state(state, embedded_linux_binary);
    }).await?;

    let task_args_json = serde_json::to_string(&task_args).unwrap_or_else(|_| "[]".to_string());

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const task = mod["{task_name}"];
            if (!task) {{
                throw new Error("Task '{task_name}' not found. Available tasks: " +
                    Object.keys(mod).filter(k => typeof mod[k] === 'function' && k !== 'default').join(', '));
            }}
            if (typeof task !== 'function') {{
                throw new Error("'{task_name}' is not a function");
            }}
            const rawArgs = {task_args_json};
            if (task.__ebdevTaskDef) {{
                const parsed = task.__ebdevTaskDef.__parseArgs(rawArgs);
                await task.__ebdevTaskDef.run(parsed);
            }} else if (rawArgs.length > 0) {{
                throw new Error("Task '{task_name}' does not accept arguments. Use defineTask() to define a task with typed arguments.");
            }} else {{
                await task();
            }}
            globalThis.__taskResult = "ok";
        }})()
    "#);

    let result_str = exec_and_read_global(&mut rt, "<run>", code, "globalThis.__taskResult").await?;

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
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"build"));
        assert!(names.contains(&"test"));
        assert!(names.contains(&"deploy"));
        assert!(!names.contains(&"VERSION"));
        assert!(!names.contains(&"default"));
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

        let result = run_task(&config_path, "build", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "nonexistent", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_fs", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_append", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_mkdir", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_rm", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_rm_recursive", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_stat_missing", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_read_error", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_write_error", Some(handle), None, HashMap::new(), b"", vec![]).await;

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

        let result = run_task(&config_path, "test_stat_size", Some(handle), None, HashMap::new(), b"", vec![]).await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "test_stat_size should succeed: {:?}", result);
    }

    #[tokio::test]
    async fn test_complete_arg() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({});

export const greet = defineTask({
    description: "Greet someone",
    args: {
        name: arg.string("Name").required().complete(async () => {
            return ["Alice", "Bob", "Charlie"];
        }),
        loud: arg.boolean("Shout"),
    },
    async run({ name, loud }) {},
});
"#).unwrap();

        let values = complete_arg(&config_path, "greet", "name").await.unwrap();
        assert_eq!(values, vec!["Alice", "Bob", "Charlie"]);
    }

    #[tokio::test]
    async fn test_complete_arg_no_complete_fn() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({});

export const greet = defineTask({
    args: {
        name: arg.string("Name").required(),
    },
    async run({ name }) {},
});
"#).unwrap();

        let values = complete_arg(&config_path, "greet", "name").await.unwrap();
        assert!(values.is_empty(), "Should return empty for arg without .complete()");
    }

    #[tokio::test]
    async fn test_complete_arg_nonexistent_task() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig } from "ebdev";
export default defineConfig({});
export async function build() {}
"#).unwrap();

        let values = complete_arg(&config_path, "nonexistent", "foo").await.unwrap();
        assert!(values.is_empty());
    }

    #[tokio::test]
    async fn test_list_tasks_completable_flag() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({});

export const deploy = defineTask({
    args: {
        target: arg.string("Target").complete(() => ["staging", "prod"]),
        count: arg.number("Count"),
    },
    async run({ target, count }) {},
});
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        let deploy = tasks.iter().find(|t| t.name == "deploy").unwrap();
        let args = deploy.args.as_ref().unwrap();

        let target_arg = args.iter().find(|a| a.name == "target").unwrap();
        assert!(target_arg.completable, "target should be completable");

        let count_arg = args.iter().find(|a| a.name == "count").unwrap();
        assert!(!count_arg.completable, "count should not be completable");
    }

    #[tokio::test]
    async fn test_parse_args_equals_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg, exec } from "ebdev";

export default defineConfig({});

export const greet = defineTask({
    args: {
        name: arg.string("Name").required(),
        loud: arg.boolean("Shout"),
    },
    async run({ name, loud }) {
        const msg = loud ? name.toUpperCase() : name;
        await exec(["echo", msg]);
    },
});
"#).unwrap();

        // Test --name=Alice (equals syntax)
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(
            &config_path, "greet", Some(handle), None, HashMap::new(), b"",
            vec!["--name=Alice".to_string()],
        ).await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "Equals syntax should work: {:?}", result);
    }

    #[tokio::test]
    async fn test_parse_args_equals_syntax_with_boolean() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg, exec } from "ebdev";

export default defineConfig({});

export const greet = defineTask({
    args: {
        name: arg.string("Name").required(),
        loud: arg.boolean("Shout"),
    },
    async run({ name, loud }) {
        const msg = loud ? name.toUpperCase() : name;
        await exec(["echo", msg]);
    },
});
"#).unwrap();

        // Test --name=Alice --loud (mixed)
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();

        let result = run_task(
            &config_path, "greet", Some(handle), None, HashMap::new(), b"",
            vec!["--name=Alice".to_string(), "--loud".to_string()],
        ).await;

        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "Mixed equals + space syntax should work: {:?}", result);
    }

    // =========================================================================
    // parseTaskArgs edge cases
    // =========================================================================

    /// Helper: run a task with given args and return the result.
    /// Uses a config that writes the parsed args as JSON to a file.
    async fn run_parse_test(config_path: &std::path::Path, task_args: Vec<String>) -> Result<(), Error> {
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();
        let result = run_task(
            config_path, "test_task", Some(handle), None, HashMap::new(), b"",
            task_args,
        ).await;
        let _ = handle_for_shutdown.shutdown();
        result
    }

    fn write_parse_test_config(dir: &std::path::Path) -> std::path::PathBuf {
        let config_path = dir.join(".ebdev.ts");
        let result_file = dir.join("result.json");
        let result_str = result_file.to_string_lossy().to_string();
        std::fs::write(&config_path, format!(r#"
import {{ defineConfig, defineTask, arg, fs }} from "ebdev";

export default defineConfig({{}});

export const test_task = defineTask({{
    args: {{
        name: arg.string("Name"),
        count: arg.number("Count"),
        env: arg.oneOf(["staging", "prod"], "Environment"),
        loud: arg.boolean("Shout"),
        requiredField: arg.string("Required").required(),
    }},
    async run(args) {{
        await fs.writeFile("{result_str}", JSON.stringify(args));
    }},
}});
"#)).unwrap();
        config_path
    }

    fn read_result(dir: &std::path::Path) -> serde_json::Value {
        let data = std::fs::read_to_string(dir.join("result.json")).unwrap();
        serde_json::from_str(&data).unwrap()
    }

    #[tokio::test]
    async fn test_parse_equals_empty_value() {
        // --name= should be treated as empty string
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--name=".into(),
        ]).await;
        assert!(result.is_ok(), "Empty value after = should be accepted: {:?}", result);
        let parsed = read_result(dir.path());
        assert_eq!(parsed["name"], "");
    }

    #[tokio::test]
    async fn test_parse_equals_multiple_equals() {
        // --name=a=b=c should split on FIRST = only → value is "a=b=c"
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--name=a=b=c".into(),
        ]).await;
        assert!(result.is_ok(), "Multiple = should work: {:?}", result);
        let parsed = read_result(dir.path());
        assert_eq!(parsed["name"], "a=b=c");
    }

    #[tokio::test]
    async fn test_parse_equals_number_type() {
        // --count=42 should parse as number
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--count=42".into(),
        ]).await;
        assert!(result.is_ok(), "Number equals should work: {:?}", result);
        let parsed = read_result(dir.path());
        assert_eq!(parsed["count"], 42);
    }

    #[tokio::test]
    async fn test_parse_equals_number_invalid() {
        // --count=abc should fail with type error
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--count=abc".into(),
        ]).await;
        assert!(result.is_err(), "Non-numeric value for number should fail");
        assert!(result.unwrap_err().to_string().contains("expects a number"));
    }

    #[tokio::test]
    async fn test_parse_equals_oneof_valid() {
        // --env=staging should work
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--env=staging".into(),
        ]).await;
        assert!(result.is_ok(), "Valid oneOf value should work: {:?}", result);
        let parsed = read_result(dir.path());
        assert_eq!(parsed["env"], "staging");
    }

    #[tokio::test]
    async fn test_parse_equals_oneof_invalid() {
        // --env=invalid should fail with validation error
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--required-field=x".into(), "--env=invalid".into(),
        ]).await;
        assert!(result.is_err(), "Invalid oneOf value should fail");
        assert!(result.unwrap_err().to_string().contains("must be one of"));
    }

    #[tokio::test]
    async fn test_parse_required_missing() {
        // Missing required_field should fail
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--name=Alice".into(),
        ]).await;
        assert!(result.is_err(), "Missing required field should fail");
        assert!(result.unwrap_err().to_string().contains("required"));
    }

    #[tokio::test]
    async fn test_parse_mixed_syntax() {
        // Mix equals and space syntax: --name=Alice --loud --count 3 --env staging
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_parse_test_config(dir.path());
        let result = run_parse_test(&config_path, vec![
            "--name=Alice".into(), "--loud".into(), "--count".into(), "3".into(),
            "--env".into(), "staging".into(), "--required-field".into(), "yes".into(),
        ]).await;
        assert!(result.is_ok(), "Mixed syntax should work: {:?}", result);
        let parsed = read_result(dir.path());
        assert_eq!(parsed["name"], "Alice");
        assert_eq!(parsed["loud"], true);
        assert_eq!(parsed["count"], 3);
        assert_eq!(parsed["env"], "staging");
        assert_eq!(parsed["requiredField"], "yes");
    }

    // =========================================================================
    // ArgBuilder .complete() chaining order
    // =========================================================================

    #[tokio::test]
    async fn test_complete_then_required() {
        // .complete(fn).required() — completeFn must survive chaining
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        name: arg.string("Name").complete(() => ["X", "Y"]).required(),
    },
    async run({ name }) {},
});
"#).unwrap();

        // Verify completable flag is set
        let tasks = list_tasks(&config_path).await.unwrap();
        let t = tasks.iter().find(|t| t.name == "t").unwrap();
        let arg = &t.args.as_ref().unwrap()[0];
        assert!(arg.completable, ".complete().required() should preserve completable");
        assert!(arg.required, "should be required");

        // Verify complete_arg returns values
        let values = complete_arg(&config_path, "t", "name").await.unwrap();
        assert_eq!(values, vec!["X", "Y"]);
    }

    #[tokio::test]
    async fn test_required_then_complete() {
        // .required().complete(fn) — same result, different order
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        name: arg.string("Name").required().complete(() => ["A", "B"]),
    },
    async run({ name }) {},
});
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        let t = tasks.iter().find(|t| t.name == "t").unwrap();
        let arg = &t.args.as_ref().unwrap()[0];
        assert!(arg.completable, ".required().complete() should preserve completable");
        assert!(arg.required, "should be required");

        let values = complete_arg(&config_path, "t", "name").await.unwrap();
        assert_eq!(values, vec!["A", "B"]);
    }

    #[tokio::test]
    async fn test_complete_then_default() {
        // .complete(fn).default("x") — both survive chaining
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        env: arg.string("Env").complete(() => ["dev", "staging"]).default("dev"),
    },
    async run({ env }) {},
});
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        let t = tasks.iter().find(|t| t.name == "t").unwrap();
        let arg = &t.args.as_ref().unwrap()[0];
        assert!(arg.completable, ".complete().default() should preserve completable");
        assert_eq!(arg.default.as_ref().unwrap(), "dev");

        let values = complete_arg(&config_path, "t", "env").await.unwrap();
        assert_eq!(values, vec!["dev", "staging"]);
    }

    #[tokio::test]
    async fn test_default_then_complete() {
        // .default("x").complete(fn) — same result
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        env: arg.string("Env").default("dev").complete(() => ["dev", "staging"]),
    },
    async run({ env }) {},
});
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        let t = tasks.iter().find(|t| t.name == "t").unwrap();
        let arg = &t.args.as_ref().unwrap()[0];
        assert!(arg.completable, ".default().complete() should preserve completable");
        assert_eq!(arg.default.as_ref().unwrap(), "dev");

        let values = complete_arg(&config_path, "t", "env").await.unwrap();
        assert_eq!(values, vec!["dev", "staging"]);
    }

    // =========================================================================
    // Completion edge cases (complete_arg)
    // =========================================================================

    #[tokio::test]
    async fn test_complete_arg_empty_result() {
        // .complete(() => []) should return empty array
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        name: arg.string("Name").complete(() => []),
    },
    async run({ name }) {},
});
"#).unwrap();

        let values = complete_arg(&config_path, "t", "name").await.unwrap();
        assert!(values.is_empty(), "Empty complete fn should return empty vec");
    }

    #[tokio::test]
    async fn test_complete_arg_fn_throws() {
        // .complete(() => { throw new Error("boom") }) should return empty array
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, arg } from "ebdev";
export default defineConfig({});

export const t = defineTask({
    args: {
        name: arg.string("Name").complete(() => { throw new Error("boom"); }),
    },
    async run({ name }) {},
});
"#).unwrap();

        let values = complete_arg(&config_path, "t", "name").await.unwrap();
        assert!(values.is_empty(), "Throwing complete fn should gracefully return empty vec");
    }
}
