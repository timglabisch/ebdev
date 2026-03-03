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
    #[serde(rename = "pickedFlags", skip_serializing_if = "Option::is_none")]
    pub picked_flags: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlagInfo {
    pub name: String,
    pub description: String,
    pub default: serde_json::Value,
    pub requires: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Vec<FlagConfigField>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlagConfigField {
    pub name: String,
    #[serde(rename = "cliName")]
    pub cli_name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub completable: bool,
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

    // Read .ebdev/flags.json and make it available to JS before module evaluation
    // (the custom Deno runtime has no Deno.readTextFileSync — only custom ops)
    let flags_path = dir.join(".ebdev/flags.json");
    let flags_json = std::fs::read_to_string(&flags_path).unwrap_or_else(|_| "{}".to_string());
    let escaped = flags_json.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "");
    rt.execute_script("<flags_init>", format!("globalThis.__ebdevSavedFlags = \"{escaped}\";"))
        .map_err(|e| Error(e.to_string()))?;

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
                        if (def.__pickedFlags) info.pickedFlags = def.__pickedFlags;
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

/// List all feature flags defined in a .ebdev.ts file
pub async fn list_flags(path: &Path) -> Result<Vec<FlagInfo>, Error> {
    let path = path.canonicalize()?;

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, None, None, HashMap::new());
    }).await?;

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const config = mod.default;
            const flags = [];
            if (config && config.__flagDefs) {{
                for (const [name, def] of Object.entries(config.__flagDefs)) {{
                    const info = {{
                        name,
                        description: def.description || "",
                        default: def.default,
                        requires: def.requires || [],
                    }};
                    if (def.config) {{
                        info.config = [];
                        for (const [fieldName, field] of Object.entries(def.config)) {{
                            const cliName = fieldName.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
                            const fieldInfo = {{
                                name: fieldName,
                                cliName,
                                type: field.type || "string",
                                description: field.description || "",
                            }};
                            if (field.default !== undefined) fieldInfo.default = field.default;
                            if (field.choices) fieldInfo.choices = field.choices;
                            if (field.completable) fieldInfo.completable = true;
                            info.config.push(fieldInfo);
                        }}
                    }}
                    flags.push(info);
                }}
            }}
            globalThis.__flags = JSON.stringify(flags);
        }})()
    "#);

    let json = exec_and_read_global(&mut rt, "<list_flags>", code, "globalThis.__flags").await?;
    serde_json::from_str(&json).map_err(|e| Error(e.to_string()))
}

/// Run the completion function for a specific config field of a flag.
/// Returns the list of completion values, or an empty vec on error.
pub async fn complete_flag_value(path: &Path, flag_name: &str, field_name: &str) -> Result<Vec<String>, Error> {
    let path = path.canonicalize()?;

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, None, None, HashMap::new());
    }).await?;

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const config = mod.default;
            if (!config || !config.__flagBuilders) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            const builder = config.__flagBuilders["{flag_name}"];
            if (!builder || !builder._config) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            const fieldBuilder = builder._config["{field_name}"];
            if (!fieldBuilder || !fieldBuilder._completeFn) {{
                globalThis.__completeResult = "[]";
                return;
            }}
            try {{
                const values = await fieldBuilder._completeFn();
                globalThis.__completeResult = JSON.stringify(Array.isArray(values) ? values : []);
            }} catch (e) {{
                globalThis.__completeResult = "[]";
            }}
        }})()
    "#);

    let json = exec_and_read_global(&mut rt, "<complete_flag>", code, "globalThis.__completeResult").await?;
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
    flag_overrides: HashMap<String, serde_json::Value>,
) -> Result<(), Error> {
    let path = path.canonicalize()?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let (mut rt, module) = create_runtime(&path, |state| {
        init_task_runner_state(state, handle, Some(dir.to_string_lossy().to_string()), env);
        init_mutagen_state(state, mutagen_path, path.clone());
        init_bridge_state(state, embedded_linux_binary);
    }).await?;

    let task_args_json = serde_json::to_string(&task_args).unwrap_or_else(|_| "[]".to_string());
    let flag_overrides_json = serde_json::to_string(&flag_overrides).unwrap_or_else(|_| "{}".to_string());

    let code = format!(r#"
        (async () => {{
            const mod = await import("{module}");
            const config = mod.default;

            // Apply flag overrides (from --with/--without)
            const overrides = {flag_overrides_json};
            if (config && config.flags && Object.keys(overrides).length > 0) {{
                for (const [k, v] of Object.entries(overrides)) {{
                    if (v === true && config.__flagDefs && config.__flagDefs[k] && config.__flagDefs[k].config) {{
                        // --with on a config flag: build default config object
                        const obj = {{}};
                        for (const [fn, field] of Object.entries(config.__flagDefs[k].config)) {{
                            obj[fn] = field.default;
                        }}
                        config.flags[k] = obj;
                    }} else {{
                        config.flags[k] = v;
                    }}
                }}
            }}

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
                let flags = {{}};
                if (task.__ebdevTaskDef.__pickedFlags && config && config.flags) {{
                    for (const k of task.__ebdevTaskDef.__pickedFlags) {{
                        flags[k] = config.flags[k];
                    }}
                }} else if (task.__ebdevTaskDef.__flags) {{
                    flags = task.__ebdevTaskDef.__flags;
                }}
                await task.__ebdevTaskDef.run(parsed, flags);
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

        let result = run_task(&config_path, "build", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "nonexistent", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_fs", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_append", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_mkdir", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_rm", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_rm_recursive", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_stat_missing", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_read_error", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_write_error", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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

        let result = run_task(&config_path, "test_stat_size", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;

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
            vec!["--name=Alice".to_string()], HashMap::new(),
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
            vec!["--name=Alice".to_string(), "--loud".to_string()], HashMap::new(),
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
            task_args, HashMap::new(),
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

    // =========================================================================
    // Feature Flags — helpers
    // =========================================================================

    /// Write a .ebdev.ts with flag definitions + a `check()` task that writes
    /// `config.flags` (or a custom expression) as JSON to result.json.
    fn write_flag_config(dir: &std::path::Path, flags_def: &str, check_expr: &str) -> std::path::PathBuf {
        let config_path = dir.join(".ebdev.ts");
        let result_path = dir.join("result.json").to_string_lossy().to_string();
        std::fs::write(&config_path, format!(
            r#"import {{ defineConfig, defineTask, flag, arg, fs }} from "ebdev";

const config = defineConfig({{
    flags: {{ {flags_def} }},
}});
export default config;

export async function check() {{
    await fs.writeFile("{result_path}", JSON.stringify({check_expr}));
}}
"#)).unwrap();
        config_path
    }

    /// Save flags.json in the .ebdev dir.
    fn save_flags(dir: &std::path::Path, json: &str) {
        std::fs::create_dir_all(dir.join(".ebdev")).unwrap();
        std::fs::write(dir.join(".ebdev/flags.json"), json).unwrap();
    }

    /// Run the `check` task with optional flag overrides and return the result JSON.
    async fn run_flag_check(config_path: &std::path::Path, overrides: HashMap<String, serde_json::Value>) -> serde_json::Value {
        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();
        let result = run_task(config_path, "check", Some(handle), None, HashMap::new(), b"", vec![], overrides).await;
        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "check task should succeed: {:?}", result);
        let result_path = config_path.parent().unwrap().join("result.json");
        serde_json::from_str(&std::fs::read_to_string(result_path).unwrap()).unwrap()
    }

    /// Shorthand: run check with no overrides.
    async fn run_flag_check_defaults(config_path: &std::path::Path) -> serde_json::Value {
        run_flag_check(config_path, HashMap::new()).await
    }

    // =========================================================================
    // Feature Flags — list_flags
    // =========================================================================

    #[tokio::test]
    async fn test_list_flags_boolean() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag } from "ebdev";
export default defineConfig({
    flags: {
        clickhouse: flag("ClickHouse Analytics").default(true),
        mailhog: flag("Mail Catcher").default(false),
    },
});
"#).unwrap();

        let flags = list_flags(&config_path).await.unwrap();
        assert_eq!(flags.len(), 2);

        let ch = flags.iter().find(|f| f.name == "clickhouse").unwrap();
        assert_eq!(ch.description, "ClickHouse Analytics");
        assert_eq!(ch.default, serde_json::Value::Bool(true));
        assert!(ch.config.is_none());
        assert!(ch.requires.is_empty());

        let mh = flags.iter().find(|f| f.name == "mailhog").unwrap();
        assert_eq!(mh.description, "Mail Catcher");
        assert_eq!(mh.default, serde_json::Value::Bool(false));
        assert!(mh.config.is_none());
    }

    #[tokio::test]
    async fn test_list_flags_with_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag, arg } from "ebdev";
export default defineConfig({
    flags: {
        search: flag("Elasticsearch").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Search engine").default("elasticsearch"),
            index: arg.string("Index name").default("main"),
        }),
    },
});
"#).unwrap();

        let flags = list_flags(&config_path).await.unwrap();
        assert_eq!(flags.len(), 1);

        let search = &flags[0];
        assert_eq!(search.name, "search");
        assert_eq!(search.description, "Elasticsearch");
        assert_eq!(search.default, serde_json::Value::Bool(true)); // .config() sets default to true

        let config = search.config.as_ref().unwrap();
        assert_eq!(config.len(), 2);

        let engine = config.iter().find(|f| f.name == "engine").unwrap();
        assert_eq!(engine.field_type, "oneOf");
        assert_eq!(engine.default, Some(serde_json::json!("elasticsearch")));
        assert_eq!(engine.choices, Some(vec!["elasticsearch".to_string(), "meilisearch".to_string()]));
        assert_eq!(engine.cli_name, "engine");

        let index = config.iter().find(|f| f.name == "index").unwrap();
        assert_eq!(index.field_type, "string");
        assert_eq!(index.default, Some(serde_json::json!("main")));
        assert!(index.choices.is_none());
    }

    #[tokio::test]
    async fn test_list_flags_with_requires() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag } from "ebdev";
export default defineConfig({
    flags: {
        tidb: flag("TiDB Cluster").default(true),
        clickhouse: flag("ClickHouse").default(true).requires("tidb"),
    },
});
"#).unwrap();

        let flags = list_flags(&config_path).await.unwrap();
        let ch = flags.iter().find(|f| f.name == "clickhouse").unwrap();
        assert_eq!(ch.requires, vec!["tidb".to_string()]);

        let tidb = flags.iter().find(|f| f.name == "tidb").unwrap();
        assert!(tidb.requires.is_empty());
    }

    #[tokio::test]
    async fn test_list_flags_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig } from "ebdev";
export default defineConfig({});
"#).unwrap();

        let flags = list_flags(&config_path).await.unwrap();
        assert!(flags.is_empty());
    }

    #[tokio::test]
    async fn test_list_flags_config_with_chained_builders() {
        // .config().default(false).requires() — all chaining variants
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag, arg } from "ebdev";
export default defineConfig({
    flags: {
        db: flag("Database").default(true),
        search: flag("ES").config({
            engine: arg.oneOf(["es", "ms"], "Engine").default("es"),
        }).default(false).requires("db"),
    },
});
"#).unwrap();

        let flags = list_flags(&config_path).await.unwrap();
        let search = flags.iter().find(|f| f.name == "search").unwrap();
        assert_eq!(search.default, serde_json::Value::Bool(false));
        assert_eq!(search.requires, vec!["db".to_string()]);
        assert!(search.config.is_some());
    }

    // =========================================================================
    // Feature Flags — resolution (defaults, saved state, dependencies)
    // =========================================================================

    #[tokio::test]
    async fn test_flag_resolution_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"clickhouse: flag("CH").default(true),
        mailhog: flag("Mail").default(false),
        search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        })"#,
            r#"{ clickhouse: config.flags.clickhouse, mailhog: config.flags.mailhog, search: config.flags.search }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["clickhouse"], true);
        assert_eq!(data["mailhog"], false);
        assert_eq!(data["search"]["engine"], "elasticsearch");
    }

    #[tokio::test]
    async fn test_flag_resolution_saved_state() {
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"clickhouse": false, "mailhog": true}"#);
        let config_path = write_flag_config(dir.path(),
            r#"clickhouse: flag("CH").default(true),
        mailhog: flag("Mail").default(false)"#,
            r#"{ clickhouse: config.flags.clickhouse, mailhog: config.flags.mailhog }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["clickhouse"], false); // overridden from default true
        assert_eq!(data["mailhog"], true);     // overridden from default false
    }

    #[tokio::test]
    async fn test_flag_resolution_config_saved_partial() {
        // Partial saved config (only engine, not index) → index falls back to default
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"search": {"engine": "meilisearch"}}"#);
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
            index: arg.string("Index").default("main"),
        })"#,
            r#"config.flags.search"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["engine"], "meilisearch"); // saved
        assert_eq!(data["index"], "main");          // default fallback
    }

    #[tokio::test]
    async fn test_flag_resolution_dependency_enable() {
        // clickhouse ON (default) requires tidb → tidb forced ON despite default=false
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"tidb: flag("TiDB").default(false),
        clickhouse: flag("CH").default(true).requires("tidb")"#,
            r#"{ tidb: config.flags.tidb, clickhouse: config.flags.clickhouse }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["tidb"], true, "tidb should be forced ON because clickhouse requires it");
        assert_eq!(data["clickhouse"], true);
    }

    #[tokio::test]
    async fn test_flag_resolution_dependency_disable() {
        // Both saved as OFF → both stay OFF
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"tidb": false, "clickhouse": false}"#);
        let config_path = write_flag_config(dir.path(),
            r#"tidb: flag("TiDB").default(true),
        clickhouse: flag("CH").default(true).requires("tidb")"#,
            r#"{ tidb: config.flags.tidb, clickhouse: config.flags.clickhouse }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["tidb"], false);
        assert_eq!(data["clickhouse"], false, "clickhouse should stay OFF when both are saved as OFF");
    }

    #[tokio::test]
    async fn test_flag_resolution_dependency_force_enable() {
        // clickhouse ON (default=true) requires tidb → tidb forced ON despite saved=false
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"tidb": false}"#);
        let config_path = write_flag_config(dir.path(),
            r#"tidb: flag("TiDB").default(true),
        clickhouse: flag("CH").default(true).requires("tidb")"#,
            r#"{ tidb: config.flags.tidb, clickhouse: config.flags.clickhouse }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["tidb"], true, "tidb should be forced ON because clickhouse requires it");
        assert_eq!(data["clickhouse"], true);
    }

    #[tokio::test]
    async fn test_flag_resolution_invalid_json() {
        // Corrupt flags.json should gracefully fallback to defaults
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"NOT VALID JSON {{{{"#);
        let config_path = write_flag_config(dir.path(),
            r#"clickhouse: flag("CH").default(true),
        mailhog: flag("Mail").default(false)"#,
            r#"{ clickhouse: config.flags.clickhouse, mailhog: config.flags.mailhog }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["clickhouse"], true, "should use default when flags.json is invalid");
        assert_eq!(data["mailhog"], false, "should use default when flags.json is invalid");
    }

    // =========================================================================
    // Feature Flags — pick()
    // =========================================================================

    #[tokio::test]
    async fn test_flag_pick() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let result_path = dir.path().join("result.json").to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"import {{ defineConfig, defineTask, flag, arg, fs }} from "ebdev";

const config = defineConfig({{
    flags: {{
        search: flag("ES").config({{
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        }}),
        clickhouse: flag("CH").default(true),
        mailhog: flag("Mail").default(false),
    }},
}});
export default config;

export const build = defineTask({{
    flags: config.pick("search", "clickhouse"),
    async run(_args, flags) {{
        await fs.writeFile("{result_path}", JSON.stringify({{
            search: flags.search,
            clickhouse: flags.clickhouse,
            hasMailhog: "mailhog" in flags,
        }}));
    }},
}});
"#)).unwrap();

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();
        let result = run_task(&config_path, "build", Some(handle), None, HashMap::new(), b"", vec![], HashMap::new()).await;
        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "Task should succeed: {:?}", result);

        let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("result.json")).unwrap()).unwrap();
        assert_eq!(data["search"]["engine"], "elasticsearch");
        assert_eq!(data["clickhouse"], true);
        assert_eq!(data["hasMailhog"], false, "mailhog should not be in picked flags");
    }

    #[tokio::test]
    async fn test_list_tasks_picked_flags() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, defineTask, flag } from "ebdev";
const config = defineConfig({
    flags: { search: flag("ES").default(true), clickhouse: flag("CH").default(true) },
});
export default config;
export const build = defineTask({
    description: "Build",
    flags: config.pick("search", "clickhouse"),
    async run() {},
});
"#).unwrap();

        let tasks = list_tasks(&config_path).await.unwrap();
        let build = tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build.picked_flags, Some(vec!["search".to_string(), "clickhouse".to_string()]));
    }

    // =========================================================================
    // Feature Flags — run_task with flag_overrides (--with/--without)
    // =========================================================================

    #[tokio::test]
    async fn test_run_task_with_flag_override_enable() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"mailhog: flag("Mail").default(false)"#,
            r#"{ mailhog: config.flags.mailhog }"#,
        );

        let mut overrides = HashMap::new();
        overrides.insert("mailhog".to_string(), serde_json::Value::Bool(true));

        let data = run_flag_check(&config_path, overrides).await;
        assert_eq!(data["mailhog"], true, "mailhog should be overridden to ON");
    }

    #[tokio::test]
    async fn test_run_task_with_flag_override_disable() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"clickhouse: flag("CH").default(true)"#,
            r#"{ clickhouse: config.flags.clickhouse }"#,
        );

        let mut overrides = HashMap::new();
        overrides.insert("clickhouse".to_string(), serde_json::Value::Bool(false));

        let data = run_flag_check(&config_path, overrides).await;
        assert_eq!(data["clickhouse"], false, "clickhouse should be overridden to OFF");
    }

    #[tokio::test]
    async fn test_run_task_with_config_flag_override() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        })"#,
            r#"{ search: config.flags.search }"#,
        );

        let mut overrides = HashMap::new();
        let mut obj = serde_json::Map::new();
        obj.insert("engine".to_string(), serde_json::Value::String("meilisearch".to_string()));
        overrides.insert("search".to_string(), serde_json::Value::Object(obj));

        let data = run_flag_check(&config_path, overrides).await;
        assert_eq!(data["search"]["engine"], "meilisearch");
    }

    #[tokio::test]
    async fn test_run_task_with_boolean_override_on_config_flag() {
        // --with search (boolean true) on a config flag should build default config object
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"search": false}"#);
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        })"#,
            r#"{ search: config.flags.search }"#,
        );

        let mut overrides = HashMap::new();
        overrides.insert("search".to_string(), serde_json::Value::Bool(true));

        let data = run_flag_check(&config_path, overrides).await;
        assert!(data["search"].is_object(), "search should be config object, not boolean. Got: {:?}", data["search"]);
        assert_eq!(data["search"]["engine"], "elasticsearch");
    }

    #[tokio::test]
    async fn test_run_task_picked_flags_with_override() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");
        let result_path = dir.path().join("result.json").to_string_lossy().to_string();

        std::fs::write(&config_path, format!(r#"import {{ defineConfig, defineTask, flag, fs }} from "ebdev";

const config = defineConfig({{
    flags: {{ clickhouse: flag("CH").default(true), mailhog: flag("Mail").default(false) }},
}});
export default config;

export const build = defineTask({{
    flags: config.pick("clickhouse"),
    async run(_args, flags) {{
        await fs.writeFile("{result_path}", JSON.stringify({{ clickhouse: flags.clickhouse }}));
    }},
}});
"#)).unwrap();

        let mut overrides = HashMap::new();
        overrides.insert("clickhouse".to_string(), serde_json::Value::Bool(false));

        let (handle, _thread) = ebdev_task_runner::run_headless(None, None, b"");
        let handle_for_shutdown = handle.clone();
        let result = run_task(&config_path, "build", Some(handle), None, HashMap::new(), b"", vec![], overrides).await;
        let _ = handle_for_shutdown.shutdown();
        assert!(result.is_ok(), "Task should succeed: {:?}", result);

        let data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("result.json")).unwrap()).unwrap();
        assert_eq!(data["clickhouse"], false, "picked flag should reflect override");
    }

    #[tokio::test]
    async fn test_run_task_with_multiple_overrides() {
        // --with mailhog --without clickhouse combined
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"clickhouse: flag("CH").default(true),
        mailhog: flag("Mail").default(false)"#,
            r#"{ clickhouse: config.flags.clickhouse, mailhog: config.flags.mailhog }"#,
        );

        let mut overrides = HashMap::new();
        overrides.insert("clickhouse".to_string(), serde_json::Value::Bool(false));
        overrides.insert("mailhog".to_string(), serde_json::Value::Bool(true));

        let data = run_flag_check(&config_path, overrides).await;
        assert_eq!(data["clickhouse"], false);
        assert_eq!(data["mailhog"], true);
    }

    // =========================================================================
    // Feature Flags — complete_flag_value
    // =========================================================================

    #[tokio::test]
    async fn test_complete_flag_value_basic() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag, arg } from "ebdev";
export default defineConfig({
    flags: {
        search: flag("ES").config({
            index: arg.string("Index").complete(async () => ["main", "products", "orders"]),
        }),
    },
});
"#).unwrap();

        let values = complete_flag_value(&config_path, "search", "index").await.unwrap();
        assert_eq!(values, vec!["main", "products", "orders"]);
    }

    #[tokio::test]
    async fn test_complete_flag_value_no_complete_fn() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag, arg } from "ebdev";
export default defineConfig({
    flags: {
        search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        }),
    },
});
"#).unwrap();

        let values = complete_flag_value(&config_path, "search", "engine").await.unwrap();
        assert!(values.is_empty(), "No .complete() = empty result");
    }

    #[tokio::test]
    async fn test_complete_flag_value_nonexistent_flag() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".ebdev.ts");

        std::fs::write(&config_path, r#"
import { defineConfig, flag } from "ebdev";
export default defineConfig({
    flags: { clickhouse: flag("CH").default(true) },
});
"#).unwrap();

        let values = complete_flag_value(&config_path, "nonexistent", "field").await.unwrap();
        assert!(values.is_empty());
    }

    // =========================================================================
    // Feature Flags — config flag with default(false) and truthiness
    // =========================================================================

    #[tokio::test]
    async fn test_config_flag_default_false() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        }).default(false)"#,
            r#"{ search: config.flags.search }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["search"], false, "config flag with default(false) should resolve to false");
    }

    #[tokio::test]
    async fn test_flag_truthiness_check() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        }),
        mailhog: flag("Mail").default(false)"#,
            r#"({
            searchTruthy: !!config.flags.search,
            mailhogTruthy: !!config.flags.mailhog,
            engine: config.flags.search ? config.flags.search.engine : undefined,
        })"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["searchTruthy"], true);
        assert_eq!(data["mailhogTruthy"], false);
        assert_eq!(data["engine"], "elasticsearch");
    }

    #[tokio::test]
    async fn test_flag_saved_false_config_flag() {
        // Config flag saved as false → resolves to false, not config object
        let dir = tempfile::tempdir().unwrap();
        save_flags(dir.path(), r#"{"search": false}"#);
        let config_path = write_flag_config(dir.path(),
            r#"search: flag("ES").config({
            engine: arg.oneOf(["elasticsearch", "meilisearch"], "Engine").default("elasticsearch"),
        })"#,
            r#"{ search: config.flags.search }"#,
        );

        let data = run_flag_check_defaults(&config_path).await;
        assert_eq!(data["search"], false);
    }
}
