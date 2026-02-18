# ebdev

Reproducible dev environments from a single TypeScript config. Pin toolchain versions, define tasks, sync files into Docker containers — all managed per-project.

- **Pinned toolchains** — Node.js, pnpm, Rust, Mutagen installed to `.ebdev/toolchain/`, isolated from system versions
- **TypeScript task runner** — Define build/dev/CI workflows as async functions with parallel execution, stages, and Docker integration
- **Interactive TUI** — Live output, collapsible stages, command palette for dynamic tasks
- **Docker bridge** — Execute commands in running containers (with PTY support) via an embedded bridge binary
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
ebdev task dev --tui
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
  },
});
```

Toolchains are installed to `.ebdev/toolchain/` relative to the config file.

## Commands

| Command | Description |
|---|---|
| `ebdev toolchain install` | Install all configured toolchains |
| `ebdev toolchain info` | Show loaded configuration |
| `ebdev run <cmd> [args]` | Run command with toolchain PATH |
| `ebdev task <name>` | Run a task (headless) |
| `ebdev task <name> --tui` | Run a task with interactive TUI |
| `ebdev tasks` | List available tasks |
| `ebdev mutagen status` | Show mutagen sync sessions |
| `ebdev mutagen terminate` | Terminate project's sync sessions |
| `ebdev remote run <container> <cmd>` | Execute command in Docker container |

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

