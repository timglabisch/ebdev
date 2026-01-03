// ebdev runtime module - provides defineConfig, presets, etc.

function normalizeToolchain(val) {
  return typeof val === "string" ? { version: val } : val;
}

export function defineConfig(config) {
  if (config.toolchain) {
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
// Task Runner API
// =============================================================================

/**
 * Execute a local command
 * @param {string[]} cmd - Command and arguments as array
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @returns {Promise<{exitCode: number, stdout: string, stderr: string}>}
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
  });
}

/**
 * Execute a shell script (supports pipes, redirects, etc.)
 * @param {string} script - Shell script to execute
 * @param {Object} [options] - Options
 * @param {string} [options.cwd] - Working directory
 * @param {Record<string, string>} [options.env] - Environment variables
 * @param {string} [options.name] - Display name for TUI
 * @returns {Promise<{exitCode: number, stdout: string, stderr: string}>}
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
   * @returns {Promise<{exitCode: number, stdout: string, stderr: string}>}
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
   * @returns {Promise<{exitCode: number, stdout: string, stderr: string}>}
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
    });
  },
};
