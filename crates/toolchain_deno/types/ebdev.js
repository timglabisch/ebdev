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
