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
}
