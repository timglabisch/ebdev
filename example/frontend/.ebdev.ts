import { defineConfig } from "ebdev";

export default defineConfig({
    mutagen: {
        sync: [{
            name: "frontend",
            target: "docker://ebdev@example-sync-target-1/var/www/frontend",
            mode: "two-way",
            stage: 1,
            ignore: [".git", "node_modules", "dist", ".next"],
            polling: { enabled: true, interval: 5 },
        }],
    },
});
