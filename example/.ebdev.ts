import { defineConfig } from "ebdev";

export default defineConfig({
    toolchain: {
        node: "22.12.0",
        pnpm: "9.15.0",
        mutagen: "0.18.1",
    },
    mutagen: {
        sync: [{
            name: "shared",
            target: "docker://ebdev@example-sync-target-1/var/www/shared",
            directory: "./shared",
            mode: "two-way",
            stage: 0,
            ignore: [".git", "node_modules"],
        }],
    },
});
