// ebdev runtime module - provides defineConfig, presets, etc.

function normalizeToolchain(val) {
  return typeof val === "string" ? { version: val } : val;
}

export function defineConfig(config) {
  if (config.toolchain) {
    if (config.toolchain.ebdev) config.toolchain.ebdev = normalizeToolchain(config.toolchain.ebdev);
    if (config.toolchain.node) config.toolchain.node = normalizeToolchain(config.toolchain.node);
    if (config.toolchain.pnpm) config.toolchain.pnpm = normalizeToolchain(config.toolchain.pnpm);
    if (config.toolchain.mutagen) config.toolchain.mutagen = normalizeToolchain(config.toolchain.mutagen);
  }

  if (config.mutagen?.sync && config.mutagen.defaults) {
    const d = config.mutagen.defaults;
    config.mutagen.sync = config.mutagen.sync.map(p => ({
      mode: d.mode, polling: d.polling, ...p,
      ignore: mergeIgnore(d.ignore, p.ignore),
    }));
  }

  return config;
}

export const presets = {
  node: {
    mode: "two-way",
    ignore: ["node_modules", ".npm", ".yarn", ".pnpm-store", "dist", "build", ".next", ".nuxt", ".cache", "coverage"],
  },
  php: {
    mode: "two-way",
    ignore: ["vendor", ".phpunit.cache", "storage/framework/cache", "storage/framework/sessions", "storage/framework/views", "bootstrap/cache"],
  },
  rust: { mode: "two-way", ignore: ["target", "Cargo.lock"] },
  python: {
    mode: "two-way",
    ignore: ["__pycache__", ".venv", "venv", ".pytest_cache", ".mypy_cache", "*.pyc", "*.pyo", ".eggs", "*.egg-info"],
  },
  go: { mode: "two-way", ignore: ["vendor", "bin"] },
};

export function mergeIgnore(...arrays) {
  const result = {};
  for (const arr of arrays) {
    if (Array.isArray(arr)) for (const item of arr) result[item] = true;
  }
  return Object.keys(result);
}

export function gitignore() {
  return [".git", ".svn", ".hg", ".DS_Store", "Thumbs.db", "*.swp", "*.swo", "*~", ".idea", ".vscode", "*.log"];
}

// For backward compatibility with test runtime (global scope)
globalThis.defineConfig = defineConfig;
globalThis.presets = presets;
globalThis.mergeIgnore = mergeIgnore;
globalThis.gitignore = gitignore;

// =============================================================================
// Interactive Mode
// =============================================================================

let interactiveMode = false;

export function enableInteractive() { interactiveMode = true; }
export function disableInteractive() { interactiveMode = false; }

// =============================================================================
// Task Runner API
// =============================================================================

/**
 * Execute a local command
 * @param {string[]} cmd - Command and arguments as array
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @param {number} [options.timeout] - Timeout in seconds (default: 300)
 * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
 * @throws {Error} If command fails or times out
 */
export async function exec(cmd, options = {}) {
  if (!Array.isArray(cmd) || cmd.length === 0) {
    throw new Error("exec: cmd must be a non-empty array");
  }
  return await Deno.core.ops.op_exec({
    cmd,
    cwd: options.cwd,
    env: options.env,
    name: options.name,
    timeout: options.timeout,
    ignore_error: false,
    interactive: options.interactive || interactiveMode,
  });
}

/**
 * Execute a local command, ignoring errors (returns result instead of throwing)
 * @param {string[]} cmd - Command and arguments as array
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @param {number} [options.timeout] - Timeout in seconds (default: 300)
 * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
 */
export async function tryExec(cmd, options = {}) {
  if (!Array.isArray(cmd) || cmd.length === 0) {
    throw new Error("tryExec: cmd must be a non-empty array");
  }
  return await Deno.core.ops.op_exec({
    cmd,
    cwd: options.cwd,
    env: options.env,
    name: options.name,
    timeout: options.timeout,
    ignore_error: true,
    interactive: options.interactive || interactiveMode,
  });
}

/**
 * Execute a shell script (supports pipes, redirects, etc.)
 * @param {string} script - Shell script to execute
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @param {number} [options.timeout] - Timeout in seconds (default: 300)
 * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
 * @throws {Error} If command fails or times out
 */
export async function shell(script, options = {}) {
  if (typeof script !== "string") {
    throw new Error("shell: script must be a string");
  }
  return await Deno.core.ops.op_shell({
    script,
    cwd: options.cwd,
    env: options.env,
    name: options.name,
    timeout: options.timeout,
    ignore_error: false,
    interactive: options.interactive || interactiveMode,
  });
}

/**
 * Execute a shell script, ignoring errors (returns result instead of throwing)
 * @param {string} script - Shell script to execute
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @param {number} [options.timeout] - Timeout in seconds (default: 300)
 * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
 */
export async function tryShell(script, options = {}) {
  if (typeof script !== "string") {
    throw new Error("tryShell: script must be a string");
  }
  return await Deno.core.ops.op_shell({
    script,
    cwd: options.cwd,
    env: options.env,
    name: options.name,
    timeout: options.timeout,
    ignore_error: true,
    interactive: options.interactive || interactiveMode,
  });
}

/**
 * Execute commands in parallel
 * @param {...(() => Promise<any>)} fns - Functions to execute in parallel
 * @returns {Promise<any[]>}
 */
export async function parallel(...fns) {
  if (fns.length === 0) {
    return [];
  }

  await Deno.core.ops.op_parallel_begin(fns.length);

  try {
    const results = await Promise.all(fns.map(fn => fn()));
    return results;
  } finally {
    await Deno.core.ops.op_parallel_end();
  }
}

/**
 * Begin a new stage - collapses previous tasks and shows stage header
 * @param {string} name - Stage name to display
 * @returns {Promise<void>}
 */
export async function stage(name) {
  if (typeof name !== "string" || name.length === 0) {
    throw new Error("stage: name must be a non-empty string");
  }
  await Deno.core.ops.op_stage(name);
}

/**
 * Docker operations
 */
export const docker = {
  /**
   * Execute a command in a running container
   * @param {string} container - Container name or ID
   * @param {string[]} cmd - Command and arguments as array
   * @param {Object} [options] - Options
   * @param {string} [options.user] - User to run as
   * @param {Record<string, string>} [options.env] - Environment variables
   * @param {string} [options.name] - Display name for TUI
   * @param {number} [options.timeout] - Timeout in seconds (default: 300)
   * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
   * @throws {Error} If command fails or times out
   */
  async exec(container, cmd, options = {}) {
    if (typeof container !== "string") {
      throw new Error("docker.exec: container must be a string");
    }
    if (!Array.isArray(cmd) || cmd.length === 0) {
      throw new Error("docker.exec: cmd must be a non-empty array");
    }
    return await Deno.core.ops.op_docker_exec({
      container,
      cmd,
      user: options.user,
      env: options.env,
      name: options.name,
      timeout: options.timeout,
      ignore_error: false,
      interactive: options.interactive || interactiveMode,
    });
  },

  /**
   * Execute a command in a running container, ignoring errors
   * @param {string} container - Container name or ID
   * @param {string[]} cmd - Command and arguments as array
   * @param {Object} [options] - Options
   * @param {string} [options.user] - User to run as
   * @param {Record<string, string>} [options.env] - Environment variables
   * @param {string} [options.name] - Display name for TUI
   * @param {number} [options.timeout] - Timeout in seconds (default: 300)
   * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
   */
  async tryExec(container, cmd, options = {}) {
    if (typeof container !== "string") {
      throw new Error("docker.tryExec: container must be a string");
    }
    if (!Array.isArray(cmd) || cmd.length === 0) {
      throw new Error("docker.tryExec: cmd must be a non-empty array");
    }
    return await Deno.core.ops.op_docker_exec({
      container,
      cmd,
      user: options.user,
      env: options.env,
      name: options.name,
      timeout: options.timeout,
      ignore_error: true,
      interactive: options.interactive || interactiveMode,
    });
  },

  /**
   * Run a command in a new container
   * @param {string} image - Docker image
   * @param {string[]} cmd - Command and arguments as array
   * @param {Object} [options] - Options
   * @param {string[]} [options.volumes] - Volume mounts
   * @param {string} [options.workdir] - Working directory in container
   * @param {string} [options.network] - Network mode
   * @param {Record<string, string>} [options.env] - Environment variables
   * @param {string} [options.name] - Display name for TUI
   * @param {number} [options.timeout] - Timeout in seconds (default: 300)
   * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
   * @throws {Error} If command fails or times out
   */
  async run(image, cmd, options = {}) {
    if (typeof image !== "string") {
      throw new Error("docker.run: image must be a string");
    }
    if (!Array.isArray(cmd) || cmd.length === 0) {
      throw new Error("docker.run: cmd must be a non-empty array");
    }
    return await Deno.core.ops.op_docker_run({
      image,
      cmd,
      volumes: options.volumes,
      workdir: options.workdir,
      network: options.network,
      env: options.env,
      name: options.name,
      timeout: options.timeout,
      ignore_error: false,
      interactive: options.interactive || interactiveMode,
    });
  },

  /**
   * Run a command in a new container, ignoring errors
   * @param {string} image - Docker image
   * @param {string[]} cmd - Command and arguments as array
   * @param {Object} [options] - Options
   * @param {string[]} [options.volumes] - Volume mounts
   * @param {string} [options.workdir] - Working directory in container
   * @param {string} [options.network] - Network mode
   * @param {Record<string, string>} [options.env] - Environment variables
   * @param {string} [options.name] - Display name for TUI
   * @param {number} [options.timeout] - Timeout in seconds (default: 300)
   * @returns {Promise<{exitCode: number, success: boolean, timedOut: boolean}>}
   */
  async tryRun(image, cmd, options = {}) {
    if (typeof image !== "string") {
      throw new Error("docker.tryRun: image must be a string");
    }
    if (!Array.isArray(cmd) || cmd.length === 0) {
      throw new Error("docker.tryRun: cmd must be a non-empty array");
    }
    return await Deno.core.ops.op_docker_run({
      image,
      cmd,
      volumes: options.volumes,
      workdir: options.workdir,
      network: options.network,
      env: options.env,
      name: options.name,
      timeout: options.timeout,
      ignore_error: true,
      interactive: options.interactive || interactiveMode,
    });
  },
};

// =============================================================================
// On-the-fly Task Registration (Command Palette)
// =============================================================================

/**
 * Registry of tasks that can be triggered from the TUI Command Palette
 * @type {Map<string, {description: string, fn: () => Promise<void>}>}
 */
const taskRegistry = new Map();

/**
 * Whether the trigger polling loop is running
 */
let triggerLoopRunning = false;

/**
 * Stop the trigger polling loop
 */
function stopTriggerLoop() {
  triggerLoopRunning = false;
}

/**
 * Start the background loop that polls for task triggers from the TUI.
 * Note: This is started automatically when the first task is registered.
 * The polling is done by the TUI/Executor, this just checks for triggers.
 */
async function startTriggerLoop() {
  if (triggerLoopRunning) return;
  triggerLoopRunning = true;

  // Run the polling loop in the background using a simple async loop
  // We rely on op_poll_task_trigger being non-blocking
  (async () => {
    while (triggerLoopRunning) {
      try {
        const triggeredName = await Deno.core.ops.op_poll_task_trigger();
        if (triggeredName) {
          const task = taskRegistry.get(triggeredName);
          if (task) {
            // Start the task as a new stage
            await Deno.core.ops.op_stage(`${triggeredName} (triggered)`);
            try {
              await task.fn();
            } catch (e) {
              console.error(`Task "${triggeredName}" failed:`, e);
            }
          }
        }
        // Yield to other tasks - the op itself is async so this provides backpressure
      } catch (e) {
        // Ignore errors during polling (e.g., if runtime is shutting down)
        break;
      }
    }
  })();
}

/**
 * Register a task that can be triggered from the TUI Command Palette
 * @param {string} name - Task name (used as identifier)
 * @param {string | (() => Promise<void>)} descOrFn - Description or task function
 * @param {(() => Promise<void>)} [maybeFn] - Task function (if description provided)
 * @returns {Promise<void>}
 *
 * @example
 * // Simple form
 * await task("fixtures", loadFixtures);
 *
 * // With description
 * await task("fixtures", "Load test fixtures", async () => {
 *   await exec(["./load-fixtures.sh"]);
 * });
 */
export async function task(name, descOrFn, maybeFn) {
  if (typeof name !== "string" || name.length === 0) {
    throw new Error("task: name must be a non-empty string");
  }

  let description;
  let fn;

  if (typeof descOrFn === "function") {
    // task(name, fn)
    description = name;
    fn = descOrFn;
  } else if (typeof descOrFn === "string" && typeof maybeFn === "function") {
    // task(name, description, fn)
    description = descOrFn;
    fn = maybeFn;
  } else {
    throw new Error("task: invalid arguments. Use task(name, fn) or task(name, description, fn)");
  }

  // Store in local registry
  taskRegistry.set(name, { description, fn });

  // Register with the TUI
  await Deno.core.ops.op_task_register(name, description);

  // Start the trigger loop if not already running
  startTriggerLoop();
}

/**
 * Unregister a task from the Command Palette
 * @param {string} name - Task name to unregister
 * @returns {Promise<void>}
 */
export async function untask(name) {
  if (typeof name !== "string" || name.length === 0) {
    throw new Error("untask: name must be a non-empty string");
  }

  taskRegistry.delete(name);
  await Deno.core.ops.op_task_unregister(name);

  // Stop the trigger loop when all tasks are removed
  if (taskRegistry.size === 0) {
    stopTriggerLoop();
  }
}

/**
 * Log a message to the task runner UI.
 * Works correctly in both headless and TUI mode.
 * Use this instead of console.log() to ensure output is displayed properly.
 * @param {string} message - Message to log
 * @returns {Promise<void>}
 */
export async function log(message) {
  await Deno.core.ops.op_log(String(message));
}

// =============================================================================
// Override console.log to use the task runner's log function
// =============================================================================

const originalConsoleLog = console.log;
const originalConsoleInfo = console.info;
const originalConsoleWarn = console.warn;
const originalConsoleError = console.error;

/**
 * Format console arguments similar to how console.log does it
 */
function formatConsoleArgs(...args) {
  return args.map(arg => {
    if (typeof arg === 'string') return arg;
    if (arg === null) return 'null';
    if (arg === undefined) return 'undefined';
    try {
      return JSON.stringify(arg, null, 2);
    } catch {
      return String(arg);
    }
  }).join(' ');
}

// Override console.log - fire and forget (don't await)
console.log = (...args) => {
  const message = formatConsoleArgs(...args);
  // Fire and forget - we don't want console.log to be async
  Deno.core.ops.op_log(message).catch(() => {
    // Fallback to original if op fails
    originalConsoleLog.apply(console, args);
  });
};

console.info = (...args) => {
  const message = formatConsoleArgs(...args);
  Deno.core.ops.op_log(`[INFO] ${message}`).catch(() => {
    originalConsoleInfo.apply(console, args);
  });
};

console.warn = (...args) => {
  const message = formatConsoleArgs(...args);
  Deno.core.ops.op_log(`[WARN] ${message}`).catch(() => {
    originalConsoleWarn.apply(console, args);
  });
};

console.error = (...args) => {
  const message = formatConsoleArgs(...args);
  Deno.core.ops.op_log(`[ERROR] ${message}`).catch(() => {
    originalConsoleError.apply(console, args);
  });
};

// =============================================================================
// Mutagen Sync API
// =============================================================================

/**
 * Reconcile mutagen sessions to the desired state.
 * Creates, updates, or terminates sessions as needed.
 * Waits until all sessions reach "watching" status.
 *
 * @param {Array<{name: string, target: string, directory: string, mode?: string, ignore?: string[]}>} sessions
 * @param {{project?: string}} [options]
 * @returns {Promise<void>}
 *
 * @example
 * // Start sync
 * await mutagenReconcile([
 *   { name: "app", target: "docker://app/www", directory: "./src" }
 * ]);
 *
 * // Cleanup all sessions
 * await mutagenReconcile([]);
 */
export async function mutagenReconcile(sessions, options = {}) {
  if (!Array.isArray(sessions)) {
    throw new Error("mutagenReconcile: sessions must be an array");
  }

  // Validate each session
  for (const session of sessions) {
    if (typeof session.name !== "string" || session.name.length === 0) {
      throw new Error("mutagenReconcile: each session must have a non-empty 'name'");
    }
    if (typeof session.target !== "string" || session.target.length === 0) {
      throw new Error("mutagenReconcile: each session must have a non-empty 'target'");
    }
    if (typeof session.directory !== "string" || session.directory.length === 0) {
      throw new Error("mutagenReconcile: each session must have a non-empty 'directory'");
    }
  }

  return await Deno.core.ops.op_mutagen_reconcile({
    sessions: sessions.map(s => ({
      name: s.name,
      target: s.target,
      directory: s.directory,
      mode: s.mode,
      ignore: s.ignore,
    })),
    project: options.project,
  });
}

/**
 * Pause all mutagen sessions belonging to this project.
 *
 * Call this before any operation that might destroy Docker containers/volumes
 * to prevent mutagen from syncing empty remote state back to local (data loss).
 * Sessions can be resumed later by calling mutagenReconcile() with the desired sessions.
 *
 * @returns {Promise<number>} Number of sessions paused
 *
 * @example
 * await mutagenPauseAll();
 * await shell("docker compose down --volumes");
 * await mutagenReconcile([]);
 */
export async function mutagenPauseAll() {
  return await Deno.core.ops.op_mutagen_pause_all();
}
