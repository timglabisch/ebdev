import { defineConfig, exec, shell, parallel, tryExec, tryShell, stage, task, untask } from "ebdev";

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
    await stage("yay");
    await task("foo", "start some foo", async () => {
        await exec(["sleep", "2"]);
    });
    await exec(["sleep", "2"]);
    await stage("parallel");

    await parallel(
        () => exec(["echo", "Task 1"]),
        () => exec(["echo", "Task 2"]),
        () => exec(["sleep", "1"]),
    );
    await stage("finish");

    await exec(["sleep", "2"]);
    console.log("All parallel tasks completed!");
}

export async function test_shell() {
    await shell("echo 'Using shell:' && date && echo 'Done!'");
}

// Test timeout feature - this will timeout after 2 seconds
export async function test_timeout() {
    console.log("Testing timeout (2s)...");
    await exec(["sleep", "10"], { timeout: 2, name: "Sleep 10s with 2s timeout" });
}

// Test error handling - this will fail and stop execution
export async function test_fail() {
    console.log("Testing error handling...");
    await exec(["ls", "/nonexistent/path"]);
    console.log("This should not be printed!");
}

// Test tryExec - command fails but execution continues
export async function test_try() {
    console.log("Testing tryExec (errors ignored)...");
    const result = await tryExec(["ls", "/nonexistent/path"]);
    console.log(`Command returned: exitCode=${result.exitCode}, success=${result.success}`);
    console.log("Execution continues after failed command!");
    await exec(["echo", "This WILL be printed!"]);
}

// Test tryShell - shell fails but execution continues
export async function test_try_shell() {
    console.log("Testing tryShell (errors ignored)...");
    const result = await tryShell("exit 42");
    console.log(`Shell returned: exitCode=${result.exitCode}, success=${result.success}`);
    console.log("Execution continues after failed shell!");
}

// Test stage functionality
export async function test_stages() {
    await stage("Build");
    await exec(["echo", "Compiling..."], { name: "Compile TypeScript" });
    await exec(["sleep", "1"], { name: "Link objects" });

    await stage("Test");
    await exec(["echo", "Running tests..."], { name: "Run unit tests" });
    await exec(["echo", "Coverage report"], { name: "Generate coverage" });

    await stage("Deploy");
    await exec(["echo", "Deploying..."], { name: "Deploy to staging" });
    await exec(["echo", "Done!"], { name: "Notify team" });
}

// Test on-the-fly task registration (Command Palette)
export async function test_tasks() {
    console.log("Testing on-the-fly task registration...");
    console.log("Press '/' to open the Command Palette and run a task!");

    // Register some tasks that can be triggered from the TUI
    await task("fixtures", "Load test fixtures into database", async () => {
        await exec(["echo", "Loading fixtures..."]);
        await exec(["sleep", "1"]);
        await exec(["echo", "Fixtures loaded!"]);
    });

    await task("clear-cache", "Clear all caches", async () => {
        await exec(["echo", "Clearing caches..."]);
        await exec(["sleep", "0.5"]);
        await exec(["echo", "Caches cleared!"]);
    });

    await task("restart", "Restart services", async () => {
        await exec(["echo", "Restarting services..."]);
        await exec(["sleep", "1"]);
        await exec(["echo", "Services restarted!"]);
    });

    await stage("Main Task");
    await exec(["echo", "Main task is running..."]);

    // Simulate a long-running task
    console.log("Waiting for 30 seconds... Press '/' to run a task!");
    await exec(["sleep", "30"], { name: "Long running process" });

    // Cleanup
    await untask("fixtures");
    await untask("clear-cache");
    await untask("restart");

    console.log("Done!");
}
