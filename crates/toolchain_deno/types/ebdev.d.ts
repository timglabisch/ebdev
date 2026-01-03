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
   * Toolchain configuration
   */
  export interface ToolchainConfig {
    /** Node.js configuration */
    node: NodeToolchain | string;
    /** pnpm configuration (optional) */
    pnpm?: PnpmToolchain | string;
    /** Mutagen configuration (optional) */
    mutagen?: MutagenToolchain | string;
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
  }

  /**
   * Options for exec and shell commands
   */
  export interface ExecOptions {
    /** Working directory */
    cwd?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Display name for TUI */
    name?: string;
    /** Timeout in seconds (default: 300) */
    timeout?: number;
  }

  /**
   * Options for docker.exec command
   */
  export interface DockerExecOptions {
    /** User to run as */
    user?: string;
    /** Environment variables */
    env?: Record<string, string>;
    /** Display name for TUI */
    name?: string;
    /** Timeout in seconds (default: 300) */
    timeout?: number;
  }

  /**
   * Options for docker.run command
   */
  export interface DockerRunOptions {
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
  };
}
