import { defineConfig, exec, shell, parallel } from "ebdev";

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

// =============================================================================
// Tasks - exported async functions that can be run with `ebdev task <name>`
// =============================================================================

export async function hello() {
    await exec(["echo", "Hello from ebdev task runner!"]);
}

export async function greet() {
    const name = "World";
    await exec(["echo", `Hello, ${name}!`]);
}

export async function info() {
    console.log("Running multiple commands...");
    await exec(["uname", "-a"]);
    await exec(["date"]);
}

export async function test_parallel() {
    console.log("Running commands in parallel...");
    await exec(["sleep", "2"]);
    await parallel(
        () => exec(["echo", "Task 1"]),
        () => exec(["echo", "Task 2"]),
        () => exec(["sleep", "1"]),
    );
    await exec(["sleep", "2"]);
    console.log("All parallel tasks completed!");
}

export async function test_shell() {
    await shell("echo 'Using shell:' && date && echo 'Done!'");
}
