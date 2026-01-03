/**
 * ebdev Configuration (TypeScript)
 *
 * This file demonstrates the TypeScript-based configuration for ebdev.
 * It provides full type safety and autocompletion in your IDE.
 */
import { defineConfig, presets, mergeIgnore } from "ebdev";

export default defineConfig({
  toolchain: {
    node: "22.12.0",
    pnpm: "9.15.0",
    mutagen: "0.18.1",
  },

  mutagen: {
    // Default settings applied to all sync projects
    defaults: {
      mode: "two-way",
      ignore: [
        // Version control
        ".git",
        ".svn",

        // OS files
        ".DS_Store",
        "Thumbs.db",

        // IDE files
        ".idea",
        ".vscode",
        "*.swp",
        "*.swo",
      ],
    },

    // Sync projects
    sync: [
      {
        // Use the Node.js preset and override specific settings
        ...presets.node,
        name: "frontend-ts",
        target: "docker://app/var/www/frontend",
        directory: "../frontend",
        stage: 0,
        // Merge preset ignore with project-specific patterns
        ignore: mergeIgnore(presets.node.ignore, [
          ".env.local",
          "tmp",
        ]),
      },
      {
        // PHP backend
        ...presets.php,
        name: "backend-ts",
        target: "docker://app/var/www/backend",
        stage: 1,
        ignore: mergeIgnore(presets.php.ignore, [
          ".env",
          "storage/logs/*",
        ]),
      },
    ],
  },
});
