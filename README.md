# ebdev

Reproducible dev environments from a single TypeScript config. Pin toolchain versions, define tasks, sync files into Docker containers — all managed per-project.

- **Pinned toolchains** — Node.js, pnpm, Rust, Mutagen, and arbitrary binaries installed to `.ebdev/toolchain/`, isolated from system versions
- **TypeScript task runner** — Define build/dev/CI workflows as async functions with parallel execution, stages, and Docker integration
- **Interactive TUI** — Live output, collapsible stages, command palette for dynamic tasks
- **Docker bridge** — Execute commands and file operations in running containers (with PTY support) via an embedded bridge binary
- **Filesystem API** — Read/write files locally or inside Docker containers without shell escaping
- **Mutagen sync** — Declarative file sync sessions with safe shutdown handling
- **Self-updating** — Binary auto-updates to the version pinned in config

## Quick Start

```bash
# Install toolchains defined in .ebdev.ts
ebdev toolchain install

# Run commands with managed toolchain
ebdev run node -v
ebdev run pnpm install
ebdev run cargo build

# Run tasks defined in .ebdev.ts
ebdev task build
ebdev task dev
```

## Configuration

Every project needs a `.ebdev.ts` in the root:

```typescript
import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.0.5",       // auto-updates binary to this version
    node: "22.12.0",
    pnpm: "9.15.0",       // optional
    rust: "1.84.0",       // optional - installs via rustup
    mutagen: "0.18.1",    // optional
    gh: "2.67.0",         // optional - managed GitHub CLI
    binary: {             // optional - arbitrary binaries via HTTP
      jq: {
        version: "1.7.1",
        url: "https://github.com/jqlang/jq/releases/download/jq-{version}/jq-{target}",
      },
    },
  },
});
```

Toolchains are installed to `.ebdev/toolchain/` relative to the config file.

### Binary Toolchains

The `binary` field lets you pin any binary that can be downloaded via HTTP. Each entry specifies a name, version, and URL template:

```typescript
binary: {
  "my-tool": {
    version: "1.0.0",
    url: "https://example.com/releases/v{version}/my-tool-{target}.tar.gz",
    binary: "bin/my-tool",  // optional: path inside archive (default: key name)
  },
},
```

**URL placeholders:**

| Placeholder | Replaced with |
|---|---|
| `{version}` | The configured version string |
| `{target}` | Platform triple (e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`) |

**Private GitHub repos** — prefix the URL with `gh:` to download with GitHub authentication:

```typescript
binary: {
  "my-private-tool": {
    version: "0.0.2",
    url: "gh:https://github.com/org/private-repo/releases/download/v{version}/tool-{target}",
  },
},
```

The `gh:` prefix resolves a token automatically (in order): `gh auth token` (system or managed gh CLI), `GITHUB_TOKEN` env, `GH_TOKEN` env. Configure a managed gh CLI via `gh: "2.67.0"` in the toolchain config or authenticate your system `gh` with `gh auth login`.

**Supported archive formats** (detected from URL extension):

- `.tar.gz` / `.tgz` — tar gzip
- `.tar.xz` — tar xz
- Plain binary (no extension match) — downloaded as-is

Binaries are installed to `.ebdev/toolchain/binary/<name>/v<version>/` and added to `PATH` by `ebdev run`.

## Commands

| Command | Description |
|---|---|
| `ebdev toolchain install` | Install all configured toolchains |
| `ebdev toolchain info` | Show loaded configuration |
| `ebdev run <cmd> [args]` | Run command with toolchain PATH |
| `ebdev task <name>` | Run a task (TUI auto-detected, headless if not a terminal) |
| `ebdev task <name> --no-tui` | Run a task in headless mode |
| `ebdev tasks` | List available tasks |
| `ebdev flags` | List feature flags and their state |
| `ebdev flag <name> on/off` | Set a feature flag |
| `ebdev flag <name>.<field> <value>` | Set a flag config field |
| `ebdev mutagen status` | Show mutagen sync sessions |
| `ebdev mutagen terminate` | Terminate project's sync sessions |
| `ebdev remote run <container> <cmd>` | Execute command in Docker container |
| `ebdev completions zsh` | Generate shell completions |

### Run Flags

```bash
ebdev run --node-version 20.0.0 node -v     # override node version
ebdev run --pnpm-version 9.14.0 pnpm -v     # override pnpm version
ebdev run --rust-version 1.83.0 rustc --version
ebdev run --mutagen-version 0.17.5 mutagen version
```

When Rust is configured, `ebdev run` sets `RUSTUP_HOME` and `CARGO_HOME` so that `rustc`, `cargo`, and `rustup` use the managed installation instead of any system Rust.

### Remote Flags

```bash
ebdev remote run mycontainer bash              # simple execution
ebdev remote run mycontainer -i vim file.txt   # interactive PTY mode
ebdev remote run mycontainer -w /app ls        # set working directory
```

## Task Runner

Tasks are exported async functions in `.ebdev.ts`:

```typescript
import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: { ebdev: "0.0.5", node: "22.12.0" },
});

export async function build() {
  await exec(["pnpm", "build"]);
}

export async function ci() {
  await stage("Lint");
  await parallel(
    () => exec(["pnpm", "lint"]),
    () => exec(["pnpm", "type-check"]),
  );

  await stage("Test");
  await exec(["pnpm", "test"]);
}

export async function dev() {
  await stage("Sync");
  await mutagenReconcile(sessions);

  await stage("Services");
  await task("reload-db", "Reload database fixtures", async () => {
    await docker.exec("app", ["php", "artisan", "db:seed"]);
  });

  await log("Ready!");
}
```

### Typed Task Arguments

Use `defineTask` with `arg` to declare typed CLI arguments. Arguments are parsed from `--` flags and passed to `run()` as a typed object.

```typescript
import { defineConfig, defineTask, arg, exec } from "ebdev";

export default defineConfig({
  toolchain: { ebdev: "0.0.5", node: "22.12.0" },
});

export const deploy = defineTask({
  description: "Deploy to environment",
  args: {
    target: arg.string("Target environment").required(),
    replicas: arg.number("Number of replicas").default(1),
    dryRun: arg.boolean("Simulate without changes"),
    logLevel: arg.oneOf(["debug", "info", "warn", "error"], "Log level").default("info"),
  },
  async run({ target, replicas, dryRun, logLevel }) {
    await exec(["./deploy.sh", target, "--replicas", String(replicas)]);
  },
});
```

```bash
ebdev task deploy -- --target staging --replicas 3 --dry-run
ebdev task deploy -- --target=staging --log-level=debug
```

**Arg types:**

| Builder | CLI syntax | Value |
|---|---|---|
| `arg.string(desc)` | `--name value` or `--name=value` | `string` |
| `arg.number(desc)` | `--count 3` or `--count=3` | `number` |
| `arg.boolean(desc)` | `--verbose` / `--no-verbose` | `boolean` |
| `arg.oneOf(["a","b"], desc)` | `--env staging` | validated `string` |

**Modifiers:** `.required()`, `.default(value)` — chainable in any order.

### Feature Flags

Define per-developer feature flags to toggle optional services or behavior. Flags are persisted locally in `.ebdev/flags.json` (gitignored) and only store deviations from defaults.

#### Defining Flags

```typescript
import { defineConfig, flag, arg } from "ebdev";

const config = defineConfig({
  toolchain: { ebdev: "0.1.0", node: "22.12.0" },
  flags: {
    clickhouse: flag("ClickHouse Analytics").default(true),
    search: flag("Elasticsearch").config({
      engine: arg.oneOf(["elasticsearch", "meilisearch"], "Search engine").default("elasticsearch"),
      index: arg.string("Search index").default("main").complete(async () => ["main", "products"]),
    }),
    mailhog: flag("Mail Catcher").default(false),
    tidb: flag("TiDB Cluster").default(true),
    analytics: flag("Analytics").default(true).requires("tidb"),
  },
});
export default config;
```

**Flag types:**

| Builder | Resolved type | Description |
|---|---|---|
| `flag("desc")` | `boolean` | Simple on/off toggle (default: `false`) |
| `flag("desc").default(true)` | `boolean` | Boolean with custom default |
| `flag("desc").config({...})` | `{ field: value } \| false` | Config object when ON, `false` when OFF |
| `.requires("other")` | — | Declares a dependency (see below) |

**Dependencies:** `.requires("other")` ensures that when a flag is enabled, its dependencies are auto-enabled too. Conversely, disabling a dependency auto-disables all flags that require it.

Config fields inside `.config({...})` use the same `arg` builders as task arguments — `arg.string()`, `arg.number()`, `arg.oneOf()`, with `.default()` and `.complete()` for shell completions.

#### Using Flags in Tasks

```typescript
export async function dev() {
  if (config.flags.search) {
    console.log(config.flags.search.engine); // "elasticsearch" | "meilisearch"
  }
  if (config.flags.clickhouse) { /* ... */ }
}
```

Config flags resolve to their config object when ON, or `false` when OFF — so a truthiness check narrows the type automatically.

#### Scoping Flags to Tasks

Use `config.pick()` to pass only relevant flags to a task:

```typescript
export const build = defineTask({
  flags: config.pick("search", "clickhouse"),
  async run(args, flags) {
    flags.search    // typed
    flags.clickhouse // typed
  },
});
```

#### CLI Commands

```bash
ebdev flags                          # List all flags and their current state
ebdev flags --json                   # Output as JSON
ebdev flag search off                # Persistently disable a flag
ebdev flag search on                 # Persistently enable a flag
ebdev flag search                    # Toggle current state
ebdev flag search.engine meilisearch # Set a config field value
```

#### One-Time Overrides

Override flags for a single task run without changing the persisted state:

```bash
ebdev task dev --with search                           # Enable for this run
ebdev task dev --without clickhouse                    # Disable for this run
ebdev task dev --with search:engine=meilisearch        # Override config values (colon syntax)
ebdev task dev --with search.engine=meilisearch        # Override config values (dot syntax)
ebdev task dev --with mailhog --without clickhouse     # Combine multiple overrides
```

#### Dynamic Shell Completions

Add `.complete()` to any non-boolean arg to provide dynamic completion values at `<TAB>` time. The function runs when the user requests shell completions and can return values computed at runtime.

```typescript
export const deploy = defineTask({
  args: {
    target: arg.string("Target environment")
      .required()
      .complete(async () => {
        // Dynamic: read from config, call API, list files, etc.
        return ["staging", "production", "dev"];
      }),
    region: arg.oneOf(["eu", "us"], "Region")
      .complete(async () => {
        // Static choices from oneOf are merged with dynamic values
        return ["eu-west-1", "us-east-1"];
      }),
  },
  async run({ target, region }) { ... },
});
```

```bash
ebdev task deploy -- --target <TAB>     # shows: staging, production, dev
ebdev task deploy -- --target=st<TAB>   # shows: staging (filtered)
ebdev task deploy -- --region <TAB>     # shows: eu, us, eu-west-1, us-east-1
```

`.complete()` is chainable with `.required()` and `.default()` in any order. The completion function should return a `string[]` (or `Promise<string[]>`). If it throws, completions degrade gracefully to static choices only.

### API Reference

#### Execution

| Function | Throws on error | Shell |
|---|---|---|
| `exec(cmd, opts?)` | yes | no |
| `tryExec(cmd, opts?)` | no | no |
| `shell(script, opts?)` | yes | yes |
| `tryShell(script, opts?)` | no | yes |

```typescript
// exec runs a command array directly (no shell interpretation)
await exec(["echo", "hello"]);

// shell runs through sh (pipes, redirects, etc.)
await shell("echo hello | tr a-z A-Z");

// try* variants return ExecResult instead of throwing
const result = await tryExec(["false"]);
// result.exitCode, result.success, result.timedOut

// stdout and stderr are captured and returned
const ver = await shell("node -v");
console.log(ver.stdout); // "v22.12.0\r\n"

// Capture output from Docker containers
const hostname = await docker.exec("app", ["hostname"]);
console.log(hostname.stdout.trim());
```

**ExecResult:**
```typescript
{
  exitCode: number,    // process exit code
  success: boolean,    // true if exit code is 0
  timedOut: boolean,   // true if the command was killed by timeout
  stdout: string,      // captured stdout (with PTY: combined stdout+stderr)
  stderr: string,      // captured stderr (with PTY: empty, since PTY merges streams)
}
```

> **Note:** Commands run through a PTY by default (for TUI rendering). In PTY mode, stdout and stderr are merged into `stdout` and `stderr` will be empty. Interactive commands (`interactive: true`) inherit the terminal directly and return empty `stdout`/`stderr`.

**ExecOptions:**
```typescript
{
  cwd?: string,                    // working directory
  env?: Record<string, string>,    // environment variables
  name?: string,                   // display name in TUI
  timeout?: number,                // seconds, default: 300
  interactive?: boolean,           // run with real terminal (suspends TUI)
  onOutput?: (data: string) => void,   // streaming callback (combined)
  onStdout?: (data: string) => void,   // streaming callback (stdout only)
  onStderr?: (data: string) => void,   // streaming callback (stderr only)
  lineBuffered?: boolean,              // deliver complete lines to callbacks
}
```

#### Streaming Output

All exec/shell/docker commands support streaming callbacks via `onOutput`, `onStdout`, and `onStderr`. Output is still captured in `result.stdout`/`result.stderr` as before.

```typescript
// React to output as it arrives
await shell("npm install", {
  onOutput: (chunk) => {
    if (chunk.includes("added")) log("Dependencies ready!");
  },
});

// Line-buffered mode: callbacks receive complete lines (no \r\n)
await shell("npm test", {
  lineBuffered: true,
  onStdout: (line) => {
    if (line.includes("FAIL")) log(`Failed: ${line}`);
  },
});

// Works with docker too
await docker.exec("app", ["php", "artisan", "migrate"], {
  lineBuffered: true,
  onOutput: (line) => log(`migrate: ${line}`),
});
```

#### Filesystem

Read and write files locally or inside Docker containers — no shell escaping needed, binary-safe via the bridge protocol.

```typescript
// Local filesystem
await fs.writeFile("/tmp/config.yaml", yamlContent);
const data = await fs.readFile("/tmp/config.yaml");
await fs.appendFile("/tmp/log.txt", "new line\n");
await fs.mkdir("/tmp/nested/dirs");                     // recursive by default
await fs.mkdir("/tmp/single", { recursive: false });    // only create leaf dir
await fs.rm("/tmp/config.yaml");                        // single file
await fs.rm("/tmp/nested", { recursive: true });        // directory tree
const exists = await fs.exists("/tmp/config.yaml");     // boolean
const stat = await fs.stat("/tmp/config.yaml");         // { exists, isFile, isDir, size }

// Docker containers (via bridge protocol)
await docker.fs.writeFile("container", "/tmp/task.yaml", yamlContent);
const content = await docker.fs.readFile("container", "/tmp/task.yaml");
await docker.fs.appendFile("container", "/var/log/app.log", "entry\n");
await docker.fs.mkdir("container", "/tmp/work/sub");
await docker.fs.rm("container", "/tmp/work", { recursive: true });
const exists = await docker.fs.exists("container", "/tmp/task.yaml");
const stat = await docker.fs.stat("container", "/tmp/task.yaml");
```

All operations throw on error (e.g. writing to a non-existent directory, reading a missing file). Use try/catch to handle errors:

```typescript
try {
  await docker.fs.writeFile("container", "/readonly/file.txt", "data");
} catch (e) {
  console.error("Write failed:", e.message);
}
```

**StatResult:**
```typescript
{
  exists: boolean,   // true if path exists
  isFile: boolean,   // true if regular file
  isDir: boolean,    // true if directory
  size: number,      // file size in bytes (0 if not exists)
}
```

#### Docker

```typescript
// Execute in running container
await docker.exec("container", ["npm", "build"]);
await docker.exec("container", ["cmd"], { user: "www-data", env: { NODE_ENV: "prod" } });

// Run in new container
await docker.run("node:22", ["npm", "--version"], {
  volumes: ["./src:/app"],
  workdir: "/app",
  network: "host",
});

// try* variants available for both
const result = await docker.tryExec("container", ["cmd"]);

// Interactive shell in a container (suspends TUI, gives real terminal)
await docker.exec("app", ["/bin/bash"], { interactive: true });
```

#### Concurrency & Structure

```typescript
// Run functions in parallel
await parallel(
  () => exec(["task1"]),
  () => exec(["task2"]),
);

// Organize output into collapsible stages
await stage("Build");
await exec(["pnpm", "build"]);

await stage("Deploy");
await exec(["./deploy.sh"]);
```

#### Interactive Commands

Commands that need a real terminal (e.g. shells, interactive editors) can use `interactive: true`.
This suspends the TUI, gives the process full stdin/stdout/stderr access, and resumes the TUI when it exits.

```typescript
// Drop into a bash shell inside a container
await exec(["bash"], { interactive: true });
await docker.exec("app", ["/bin/bash"], { interactive: true });

// Interactive docker run
await docker.run("ubuntu:24.04", ["/bin/bash"], { interactive: true });
```

For tasks where every command needs a real terminal, use `enableInteractive()` instead of marking each command:

```typescript
export async function cli() {
  enableInteractive();
  await docker.exec("app", ["/bin/bash"]);
  // all commands here run interactively
  disableInteractive(); // optional: restore default
}
```

#### Dynamic Tasks (TUI only)

Press `/` in TUI mode to open the Command Palette.

```typescript
// Register a task triggerable from TUI
await task("seed-db", "Seed the database", async () => {
  await docker.exec("app", ["php", "artisan", "db:seed"]);
});

// Unregister
await untask("seed-db");
```

#### Logging

```typescript
await log("message");  // preferred over console.log for TUI compatibility
```

### Mutagen Sync

```typescript
import { mutagenReconcile, mutagenPauseAll, MutagenSession } from "ebdev";

const sessions: MutagenSession[] = [
  {
    name: "app",
    target: "docker://container/var/www",
    directory: "./src",
    mode: "two-way",             // "two-way" | "one-way-create" | "one-way-replica"
    ignore: [".git", "node_modules", "dist"],
  },
];

// Create/update sessions to match desired state
await mutagenReconcile(sessions);

// Terminate all sessions (cleanup)
await mutagenReconcile([]);

// Pause all project sessions (returns number of sessions paused)
await mutagenPauseAll();
```

#### Safe shutdown pattern

Always pause mutagen sessions **before** removing Docker containers/volumes.
Otherwise mutagen may see empty remote endpoints and sync deletions back to local.

```typescript
export async function down() {
  await mutagenPauseAll();                                          // 1. stop syncing
  await shell("docker compose down --volumes --remove-orphans");    // 2. safe to remove
  await mutagenReconcile([]);                                       // 3. clean up sessions
}
```

`mutagenReconcile(sessions)` automatically resumes previously paused sessions that
match the desired state, so calling `mutagenPauseAll()` before `mutagenReconcile(sessions)`
is always safe (e.g. on restart after Ctrl+C).

## Shell Completions

```bash
./ebdev completions zsh >> ~/.zshrc    # zsh
./ebdev completions bash >> ~/.bashrc  # bash
./ebdev completions fish >> ~/.config/fish/config.fish  # fish
```

Restart your shell. Completions work with `./ebdev` wrapper scripts in any project:

- `./ebdev task <TAB>` — task names (with descriptions)
- `./ebdev task deploy -- --<TAB>` — flag names from `defineTask` args
- `./ebdev task deploy -- --target <TAB>` — dynamic values from `.complete()`
- `./ebdev task deploy -- --target=st<TAB>` — filtered values (equals syntax)
- `./ebdev flag <TAB>` — flag names and dotted config fields (e.g. `search.engine`)
- `./ebdev task dev --with <TAB>` — available flag names
- `./ebdev task dev --without <TAB>` — available flag names

## Self-Update

ebdev auto-updates when the configured version differs from the running binary:

1. Reads `toolchain.ebdev` from `.ebdev.ts`
2. Downloads matching release from GitHub
3. Replaces own binary atomically
4. Re-executes the original command

Set `EBDEV_SKIP_SELF_UPDATE=1` to disable.

## Build from Source

```bash
# Build Linux bridge binary (runs in Docker containers)
make build-linux           # x86_64
make build-linux-arm64     # aarch64

# Build release binary (macOS, with embedded Linux bridge)
make build
```

