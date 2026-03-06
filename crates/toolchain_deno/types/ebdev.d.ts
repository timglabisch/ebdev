/**
 * ebdev Configuration Type Definitions
 *
 * This file provides TypeScript types for ebdev configuration files (.ebdev.ts)
 */

declare module "ebdev" {
  /**
   * Sync mode for mutagen sessions
   */
  export type SyncMode = "two-way" | "one-way-create" | "one-way-replica";

  /**
   * Polling configuration for mutagen sessions
   */
  export interface PollingConfig {
    /** Enable polling-based watching (useful for network filesystems) */
    enabled?: boolean;
    /** Polling interval in seconds */
    interval?: number;
  }

  /**
   * Mutagen sync project configuration
   */
  export interface MutagenSyncProject {
    /** Unique name for this sync project */
    name: string;
    /** Target URL (e.g., "docker://container/path") */
    target: string;
    /** Local directory to sync (relative to config file) */
    directory?: string;
    /** Sync mode */
    mode?: SyncMode;
    /** Stage number for ordered sync (lower numbers sync first) */
    stage?: number;
    /** Patterns to ignore during sync */
    ignore?: string[];
    /** Polling configuration */
    polling?: PollingConfig;
  }

  /**
   * Node.js toolchain configuration
   */
  export interface NodeToolchain {
    /** Node.js version (e.g., "22.12.0") */
    version: string;
  }

  /**
   * pnpm toolchain configuration
   */
  export interface PnpmToolchain {
    /** pnpm version (e.g., "9.15.0") */
    version: string;
  }

  /**
   * Mutagen toolchain configuration
   */
  export interface MutagenToolchain {
    /** Mutagen version (e.g., "0.18.1") */
    version: string;
  }

  /**
   * ebdev self-update toolchain configuration
   */
  export interface EbdevToolchain {
    /** ebdev version (e.g., "0.1.0") */
    version: string;
  }

  /**
   * Binary toolchain configuration
   */
  export interface BinaryToolchain {
    /** Version string (e.g., "1.0.26") */
    version: string;
    /** URL template with {version} and {target} placeholders */
    url: string;
    /** Path to binary inside archive (default: key name) */
    binary?: string;
  }

  /**
   * Toolchain configuration
   */
  export interface ToolchainConfig {
    /** ebdev self-update configuration (required) */
    ebdev: EbdevToolchain | string;
    /** Node.js configuration */
    node: NodeToolchain | string;
    /** pnpm configuration (optional) */
    pnpm?: PnpmToolchain | string;
    /** Mutagen configuration (optional) */
    mutagen?: MutagenToolchain | string;
    /** GitHub CLI version — managed gh installation (optional) */
    gh?: string;
    /** Binary toolchains (optional). Use "gh:" URL prefix for private GitHub repos. */
    binary?: Record<string, BinaryToolchain>;
  }

  /**
   * Default settings for mutagen sync projects
   */
  export interface MutagenDefaults {
    /** Default sync mode */
    mode?: SyncMode;
    /** Default ignore patterns applied to all projects */
    ignore?: string[];
    /** Default polling configuration */
    polling?: PollingConfig;
  }

  /**
   * Mutagen configuration
   */
  export interface MutagenConfig {
    /** Default settings for all sync projects */
    defaults?: MutagenDefaults;
    /** List of sync projects */
    sync?: MutagenSyncProject[];
  }

  /**
   * Complete ebdev configuration
   */
  export interface EbdevConfig {
    /** Toolchain configuration */
    toolchain: ToolchainConfig;
    /** Mutagen sync configuration */
    mutagen?: MutagenConfig;
    /** Feature flags */
    flags?: Record<string, BooleanFlagBuilder | ConfigFlagBuilder<any>>;
  }

  // =============================================================================
  // Feature Flags
  // =============================================================================

  export interface BooleanFlagBuilder {
    default(value: boolean): BooleanFlagBuilder;
    requires(...flags: string[]): BooleanFlagBuilder;
    config<T extends Record<string, StringArgBuilder | NumberArgBuilder | OneOfArgBuilder<any>>>(
      schema: T
    ): ConfigFlagBuilder<T>;
  }

  export interface ConfigFlagBuilder<T extends Record<string, any>> {
    default(value: boolean): ConfigFlagBuilder<T>;
    requires(...flags: string[]): ConfigFlagBuilder<T>;
  }

  export function flag(description: string): BooleanFlagBuilder;

  type InferArgValue<T> =
    T extends OneOfArgBuilder<infer U> ? U :
    T extends StringArgBuilder ? string :
    T extends NumberArgBuilder ? number :
    never;

  type InferFlagConfig<T> = { [K in keyof T]: InferArgValue<T[K]> };

  type InferFlag<T> =
    T extends BooleanFlagBuilder ? boolean :
    T extends ConfigFlagBuilder<infer C> ? InferFlagConfig<C> | false :
    never;

  type InferFlags<F> = { [K in keyof F]: InferFlag<F[K]> };

  interface ResolvedConfig<F extends Record<string, BooleanFlagBuilder | ConfigFlagBuilder<any>>> extends Omit<EbdevConfig, 'flags'> {
    flags: InferFlags<F>;
    pick<K extends keyof F>(...keys: K[]): { [P in K]: InferFlag<F[P]> };
  }

  /**
   * Creates a type-safe ebdev configuration
   *
   * @example
   * ```typescript
   * import { defineConfig, presets } from "ebdev";
   *
   * export default defineConfig({
   *   toolchain: {
   *     node: "22.12.0",
   *     pnpm: "9.15.0",
   *     mutagen: "0.18.1",
   *   },
   *   mutagen: {
   *     defaults: {
   *       ignore: [".git", ".DS_Store", "node_modules"],
   *     },
   *     sync: [
   *       {
   *         ...presets.node,
   *         name: "frontend",
   *         target: "docker://app/var/www/frontend",
   *       },
   *     ],
   *   },
   * });
   * ```
   */
  export function defineConfig(config: EbdevConfig): EbdevConfig;
  export function defineConfig<F extends Record<string, BooleanFlagBuilder | ConfigFlagBuilder<any>>>(
    config: EbdevConfig & { flags: F }
  ): ResolvedConfig<F>;

  /**
   * Preset configurations for common project types
   */
  export const presets: {
    /**
     * Node.js project preset with common ignore patterns
     */
    node: Partial<MutagenSyncProject>;

    /**
     * PHP project preset with common ignore patterns
     */
    php: Partial<MutagenSyncProject>;

    /**
     * Rust project preset with common ignore patterns
     */
    rust: Partial<MutagenSyncProject>;

    /**
     * Python project preset with common ignore patterns
     */
    python: Partial<MutagenSyncProject>;

    /**
     * Go project preset with common ignore patterns
     */
    go: Partial<MutagenSyncProject>;
  };

  /**
   * Utility to read .gitignore patterns from a file
   *
   * @param path - Path to the .gitignore file (relative to config)
   * @returns Array of ignore patterns
   */
  export function gitignore(path?: string): string[];

  /**
   * Utility to merge multiple ignore lists
   *
   * @param lists - Arrays of ignore patterns to merge
   * @returns Combined array of unique ignore patterns
   */
  export function mergeIgnore(...lists: (string[] | undefined)[]): string[];

  // =============================================================================
  // Filesystem API
  // =============================================================================

  export interface StatResult {
    exists: boolean;
    isFile: boolean;
    isDir: boolean;
    size: number;
  }

  export interface MkdirOptions {
    /** Create parent directories as needed (default: true) */
    recursive?: boolean;
  }

  export interface RemoveOptions {
    /** Remove directories and their contents recursively (default: false) */
    recursive?: boolean;
  }

  export const fs: {
    writeFile(path: string, content: string): Promise<void>;
    readFile(path: string): Promise<string>;
    appendFile(path: string, content: string): Promise<void>;
    mkdir(path: string, options?: MkdirOptions): Promise<void>;
    rm(path: string, options?: RemoveOptions): Promise<void>;
    exists(path: string): Promise<boolean>;
    stat(path: string): Promise<StatResult>;
  };

  // =============================================================================
  // defineTask / arg — Typed Task Arguments
  // =============================================================================

  export interface StringArgBuilder {
    required(): StringArgBuilder;
    default(value: string): StringArgBuilder;
    complete(fn: () => Promise<string[]> | string[]): StringArgBuilder;
    /** @internal */ _type: "string";
    /** @internal */ _required: boolean;
    /** @internal */ _default: string | undefined;
  }

  export interface NumberArgBuilder {
    required(): NumberArgBuilder;
    default(value: number): NumberArgBuilder;
    complete(fn: () => Promise<string[]> | string[]): NumberArgBuilder;
    /** @internal */ _type: "number";
    /** @internal */ _required: boolean;
    /** @internal */ _default: number | undefined;
  }

  export interface BooleanArgBuilder {
    required(): BooleanArgBuilder;
    default(value: boolean): BooleanArgBuilder;
    /** @internal */ _type: "boolean";
    /** @internal */ _required: boolean;
    /** @internal */ _default: boolean | undefined;
  }

  export interface OneOfArgBuilder<T extends string> {
    required(): OneOfArgBuilder<T>;
    default(value: T): OneOfArgBuilder<T>;
    complete(fn: () => Promise<string[]> | string[]): OneOfArgBuilder<T>;
    /** @internal */ _type: "oneOf";
    /** @internal */ _required: boolean;
    /** @internal */ _default: T | undefined;
    /** @internal */ _choices: T[];
  }

  export const arg: {
    string(description?: string): StringArgBuilder;
    number(description?: string): NumberArgBuilder;
    boolean(description?: string): BooleanArgBuilder;
    oneOf<T extends string>(choices: readonly T[], description?: string): OneOfArgBuilder<T>;
  };

  type InferArgType<T> =
    T extends StringArgBuilder ? (T extends { _required: true } ? string : T extends { _default: string } ? string : string | undefined) :
    T extends NumberArgBuilder ? (T extends { _required: true } ? number : T extends { _default: number } ? number : number | undefined) :
    T extends BooleanArgBuilder ? boolean :
    T extends OneOfArgBuilder<infer U> ? (T extends { _required: true } ? U : T extends { _default: U } ? U : U | undefined) :
    never;

  type InferArgs<T> = {
    [K in keyof T]: InferArgType<T[K]>;
  };

  export interface TaskDefinition<
    TArgs extends Record<string, StringArgBuilder | NumberArgBuilder | BooleanArgBuilder | OneOfArgBuilder<any>>,
    TFlags = {}
  > {
    description?: string;
    args?: TArgs;
    flags?: TFlags;
    run(args: InferArgs<TArgs>, flags: TFlags): Promise<void>;
  }

  export function defineTask<
    TArgs extends Record<string, StringArgBuilder | NumberArgBuilder | BooleanArgBuilder | OneOfArgBuilder<any>>,
    TFlags = {}
  >(
    config: TaskDefinition<TArgs, TFlags>
  ): (() => Promise<void>) & { __ebdevTaskDef: unknown };

  // =============================================================================
  // Interactive Mode
  // =============================================================================

  /** Enable interactive mode — all subsequent commands get real terminal access (suspends TUI) */
  export function enableInteractive(): void;

  /** Disable interactive mode — commands use PTY capture again */
  export function disableInteractive(): void;

  // =============================================================================
  // UI API
  // =============================================================================

  export const ui: {
    /** Enable compact mode — hides sidebar, output uses full width */
    enableCompactMode(): Promise<void>;
    /** Disable compact mode — restores sidebar */
    disableCompactMode(): Promise<void>;
  };

  // =============================================================================
  // Task Runner API
  // =============================================================================

  /**
   * Result of command execution
   */
  export interface ExecResult {
    /** Exit code of the command */
    exitCode: number;
    /** Whether the command succeeded (exit code 0) */
    success: boolean;
    /** Whether the command timed out */
    timedOut: boolean;
    /** Captured stdout output (with PTY: combined stdout+stderr) */
    stdout: string;
    /** Captured stderr output (with PTY: empty, since PTY merges streams) */
    stderr: string;
  }

  /**
   * Streaming callback options (available on all exec/shell/docker commands)
   */
  export interface StreamingOptions {
    /** Callback for each output chunk (combined stdout+stderr) */
    onOutput?: (data: string) => void;
    /** Callback for each stdout chunk */
    onStdout?: (data: string) => void;
    /** Callback for each stderr chunk */
    onStderr?: (data: string) => void;
    /** When true, callbacks receive complete lines instead of raw chunks (strips \\r\\n) */
    lineBuffered?: boolean;
  }

  /**
   * Options for exec and shell commands
   */
  export interface ExecOptions extends StreamingOptions {
    /** Working directory */
    cwd?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Display name for TUI */
    name?: string;
    /** Timeout in seconds (default: 300) */
    timeout?: number;
    /** Run interactively with real terminal (suspends TUI) */
    interactive?: boolean;
  }

  /**
   * Options for docker.exec command
   */
  export interface DockerExecOptions extends StreamingOptions {
    /** User to run as */
    user?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Display name for TUI */
    name?: string;
    /** Timeout in seconds (default: 300) */
    timeout?: number;
    /** Run interactively with real terminal (suspends TUI) */
    interactive?: boolean;
  }

  /**
   * Options for docker.run command
   */
  export interface DockerRunOptions extends StreamingOptions {
    /** Volume mounts */
    volumes?: string[];
    /** Working directory in container */
    workdir?: string;
    /** Network mode */
    network?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Display name for TUI */
    name?: string;
    /** Timeout in seconds (default: 300) */
    timeout?: number;
    /** Run interactively with real terminal (suspends TUI) */
    interactive?: boolean;
  }

  /**
   * Execute a local command
   * @param cmd - Command and arguments as array
   * @param options - Options
   * @throws Error if command fails or times out
   */
  export function exec(cmd: string[], options?: ExecOptions): Promise<ExecResult>;

  /**
   * Execute a local command, ignoring errors (returns result instead of throwing)
   * @param cmd - Command and arguments as array
   * @param options - Options
   */
  export function tryExec(cmd: string[], options?: ExecOptions): Promise<ExecResult>;

  /**
   * Execute a shell script (supports pipes, redirects, etc.)
   * @param script - Shell script to execute
   * @param options - Options
   * @throws Error if command fails or times out
   */
  export function shell(script: string, options?: ExecOptions): Promise<ExecResult>;

  /**
   * Execute a shell script, ignoring errors (returns result instead of throwing)
   * @param script - Shell script to execute
   * @param options - Options
   */
  export function tryShell(script: string, options?: ExecOptions): Promise<ExecResult>;

  /**
   * Execute commands in parallel
   * @param fns - Functions to execute in parallel
   * @returns Array of results from each function
   */
  export function parallel<T extends (() => Promise<any>)[]>(
    ...fns: T
  ): Promise<{ [K in keyof T]: Awaited<ReturnType<T[K]>> }>;

  /**
   * Begin a new stage in the task runner
   * Collapses previous tasks and displays a stage divider with the given name
   * @param name - The name of the stage to display
   */
  export function stage(name: string): Promise<void>;

  /**
   * Docker operations
   */
  export const docker: {
    /**
     * Execute a command in a running container
     * @param container - Container name or ID
     * @param cmd - Command and arguments as array
     * @param options - Options
     * @throws Error if command fails or times out
     */
    exec(container: string, cmd: string[], options?: DockerExecOptions): Promise<ExecResult>;

    /**
     * Execute a command in a running container, ignoring errors
     * @param container - Container name or ID
     * @param cmd - Command and arguments as array
     * @param options - Options
     */
    tryExec(container: string, cmd: string[], options?: DockerExecOptions): Promise<ExecResult>;

    /**
     * Run a command in a new container
     * @param image - Docker image
     * @param cmd - Command and arguments as array
     * @param options - Options
     * @throws Error if command fails or times out
     */
    run(image: string, cmd: string[], options?: DockerRunOptions): Promise<ExecResult>;

    /**
     * Run a command in a new container, ignoring errors
     * @param image - Docker image
     * @param cmd - Command and arguments as array
     * @param options - Options
     */
    tryRun(image: string, cmd: string[], options?: DockerRunOptions): Promise<ExecResult>;

    /**
     * Filesystem operations inside Docker containers (via bridge protocol)
     */
    fs: {
      writeFile(container: string, path: string, content: string): Promise<void>;
      readFile(container: string, path: string): Promise<string>;
      appendFile(container: string, path: string, content: string): Promise<void>;
      mkdir(container: string, path: string, options?: MkdirOptions): Promise<void>;
      rm(container: string, path: string, options?: RemoveOptions): Promise<void>;
      exists(container: string, path: string): Promise<boolean>;
      stat(container: string, path: string): Promise<StatResult>;
    };
  };

  // =============================================================================
  // On-the-fly Task Registration (Command Palette)
  // =============================================================================

  /**
   * Task function type
   */
  export type TaskFn = () => Promise<void>;

  /**
   * Register a task that can be triggered from the TUI Command Palette.
   * Press `/` in the TUI to open the Command Palette and select a task.
   *
   * @param name - Task name (used as identifier and display name if no description)
   * @param fn - Task function to execute when triggered
   *
   * @example
   * ```typescript
   * await task("fixtures", async () => {
   *   await exec(["./load-fixtures.sh"]);
   * });
   * ```
   */
  export function task(name: string, fn: TaskFn): Promise<void>;

  /**
   * Register a task that can be triggered from the TUI Command Palette.
   * Press `/` in the TUI to open the Command Palette and select a task.
   *
   * @param name - Task name (used as identifier)
   * @param description - Description shown in the Command Palette
   * @param fn - Task function to execute when triggered
   *
   * @example
   * ```typescript
   * await task("fixtures", "Load test fixtures into database", async () => {
   *   await docker.exec("app", ["php", "artisan", "db:seed"]);
   * });
   * ```
   */
  export function task(name: string, description: string, fn: TaskFn): Promise<void>;

  /**
   * Unregister a task from the Command Palette.
   *
   * @param name - Task name to unregister
   *
   * @example
   * ```typescript
   * // Register a task
   * await task("fixtures", loadFixtures);
   *
   * // Later, remove it
   * await untask("fixtures");
   * ```
   */
  export function untask(name: string): Promise<void>;

  /**
   * Log a message to the task runner UI.
   * Works correctly in both headless and TUI mode.
   * Use this instead of console.log() to ensure output is displayed properly.
   *
   * @param message - Message to log
   *
   * @example
   * ```typescript
   * await log("Running commands in parallel...");
   * await parallel(
   *   () => exec(["echo", "Task 1"]),
   *   () => exec(["echo", "Task 2"]),
   * );
   * await log("All tasks completed!");
   * ```
   */
  export function log(message: string): Promise<void>;

  // =============================================================================
  // Mutagen Sync API
  // =============================================================================

  /**
   * Mutagen session configuration for file synchronization
   */
  export interface MutagenSession {
    /** Unique name for this sync session */
    name: string;
    /** Target URL (e.g., "docker://container/path") */
    target: string;
    /** Local directory to sync (relative to config file) */
    directory: string;
    /** Sync mode (default: "two-way") */
    mode?: SyncMode;
    /** Patterns to ignore during sync */
    ignore?: string[];
    /** Polling configuration (for network filesystems or when native watching doesn't work) */
    polling?: PollingConfig;
  }

  /**
   * Options for mutagenReconcile
   */
  export interface MutagenReconcileOptions {
    /**
     * Project identifier for session namespacing.
     * Sessions are named "{project}-{session.name}".
     * Default: CRC32 hash of the absolute .ebdev.ts path.
     */
    project?: string;
  }

  /**
   * Reconcile mutagen sessions to the desired state.
   * Creates, updates, or terminates sessions as needed.
   * Waits until all sessions reach "watching" status.
   *
   * @param sessions - Desired session configurations
   * @param options - Reconcile options
   *
   * @example
   * ```typescript
   * const sessions: MutagenSession[] = [
   *   {
   *     name: "app",
   *     target: "docker://app/var/www",
   *     directory: "./src",
   *     mode: "two-way",
   *     ignore: [".git", "node_modules"],
   *   },
   * ];
   *
   * // Start sync
   * await mutagenReconcile(sessions);
   *
   * // Later: cleanup all sessions
   * await mutagenReconcile([]);
   * ```
   */
  export function mutagenReconcile(
    sessions: MutagenSession[],
    options?: MutagenReconcileOptions
  ): Promise<void>;

  /**
   * Pause all mutagen sessions belonging to this project.
   *
   * Call this before any operation that might destroy Docker containers/volumes
   * to prevent mutagen from syncing empty remote state back to local (data loss).
   * Sessions can be resumed later by calling `mutagenReconcile()` with the desired sessions.
   *
   * @returns Number of sessions paused
   */
  export function mutagenPauseAll(): Promise<number>;
}
