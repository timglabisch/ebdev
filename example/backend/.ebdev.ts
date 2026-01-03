import { defineConfig } from "ebdev";

export default defineConfig({
    mutagen: {
        sync: [
            {
                name: "backend-src",
                target: "docker://ebdev@example-sync-target-1/var/www/backend",
                mode: "two-way",
                stage: 1,
                ignore: [".git", "vendor", "var", ".env"],
            },
            {
                name: "backend-config",
                target: "docker://ebdev@example-sync-target-1/var/www/backend/config",
                directory: "./config",
                mode: "one-way-replica",
                stage: 2,
                ignore: [".git"],
            },
        ],
    },
});
