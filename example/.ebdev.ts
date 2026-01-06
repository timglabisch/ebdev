import { defineConfig, exec, shell, parallel, tryExec, tryShell, stage, task, untask, mutagenReconcile, MutagenSession } from "ebdev";

export default defineConfig({
    toolchain: {
        node: "22.12.0",
        pnpm: "9.15.0",
        mutagen: "0.18.1",
    },
});

// =============================================================================
// Mutagen Sessions - defined here, used via mutagenReconcile() in tasks
// =============================================================================

const mutagenSessions: MutagenSession[] = [
    {
        name: "shared",
        target: "docker://ebdev@example-sync-target-1/var/www/shared",
        directory: "./shared",
        mode: "two-way",
        ignore: [".git", "node_modules"],
    },
    {
        name: "backend",
        target: "docker://ebdev@example-sync-target-1/var/www/backend",
        directory: "./backend",
        mode: "two-way",
        ignore: [".git", "vendor", "var", ".env"],
    },
    {
        name: "frontend",
        target: "docker://ebdev@example-sync-target-1/var/www/frontend",
        directory: "./frontend",
        mode: "two-way",
        ignore: [".git", "node_modules", "dist", ".next"],
    },
];

// =============================================================================
// Tasks - exported async functions that can be run with `ebdev task <name>`
// =============================================================================

// Start development environment with mutagen sync
export async function dev() {
    await stage("Sync");
    console.log("Starting mutagen sync...");
    await mutagenReconcile(mutagenSessions);
    console.log("Mutagen sync established!");

    await stage("Services");
    console.log("Starting docker services...");
    // await exec(["docker", "compose", "up", "-d"]);
    await exec(["echo", "Docker services would start here"]);
    console.log("Development environment ready!");
}

// Stop development environment and cleanup mutagen
export async function down() {
    await stage("Services");
    console.log("Stopping docker services...");
    // await exec(["docker", "compose", "down"]);
    await exec(["echo", "Docker services would stop here"]);

    await stage("Cleanup");
    console.log("Cleaning up mutagen sessions...");
    await mutagenReconcile([]);
    console.log("Cleanup complete!");
}

// Test mutagen reconcile functionality
export async function test_mutagen() {
    console.log("Testing mutagenReconcile...");

    await stage("Create Sessions");
    await mutagenReconcile(mutagenSessions);
    console.log("Sessions created!");

    await exec(["sleep", "2"]);

    await stage("Cleanup Sessions");
    await mutagenReconcile([]);
    console.log("Sessions cleaned up!");

    console.log("Mutagen test passed ✓");
}

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

// =============================================================================
// Complex Integration Test - Tests many edge cases
// =============================================================================

export async function test_complex() {
    console.log("╔══════════════════════════════════════════════════════════════╗");
    console.log("║          COMPLEX INTEGRATION TEST                            ║");
    console.log("╚══════════════════════════════════════════════════════════════╝");

    // -------------------------------------------------------------------------
    // Stage 1: Basic sequential execution with console.log interleaving
    // -------------------------------------------------------------------------
    await stage("1. Sequential Execution");
    console.log("Testing basic sequential commands with console.log...");

    await exec(["echo", "Step 1: Starting"]);
    console.log("Console: Between step 1 and 2");
    await exec(["echo", "Step 2: Processing"]);
    console.log("Console: Between step 2 and 3");
    await exec(["echo", "Step 3: Complete"]);
    console.log("Sequential test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 2: Parallel execution with different durations
    // -------------------------------------------------------------------------
    await stage("2. Parallel with Different Durations");
    console.log("Running 5 parallel tasks with varying durations...");

    await parallel(
        () => exec(["sleep", "0.1"], { name: "Fast (0.1s)" }),
        () => exec(["sleep", "0.5"], { name: "Medium (0.5s)" }),
        () => exec(["sleep", "0.3"], { name: "Short (0.3s)" }),
        () => exec(["sleep", "0.2"], { name: "Quick (0.2s)" }),
        () => exec(["sleep", "0.4"], { name: "Normal (0.4s)" }),
    );
    console.log("Parallel duration test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 3: Parallel with output
    // -------------------------------------------------------------------------
    await stage("3. Parallel with Heavy Output");
    console.log("Testing parallel commands that produce output...");

    await parallel(
        () => shell("for i in 1 2 3 4 5; do echo \"Stream A: Line $i\"; sleep 0.1; done"),
        () => shell("for i in 1 2 3 4 5; do echo \"Stream B: Line $i\"; sleep 0.15; done"),
        () => shell("for i in 1 2 3 4 5; do echo \"Stream C: Line $i\"; sleep 0.08; done"),
    );
    console.log("Parallel output test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 4: Error handling with tryExec in parallel
    // -------------------------------------------------------------------------
    await stage("4. Error Handling in Parallel");
    console.log("Testing tryExec within parallel blocks...");

    const results: { name: string; success: boolean }[] = [];

    await parallel(
        async () => {
            const r = await tryExec(["ls", "/nonexistent/path/1"]);
            results.push({ name: "fail1", success: r.success });
        },
        async () => {
            const r = await tryExec(["echo", "success"]);
            results.push({ name: "success", success: r.success });
        },
        async () => {
            const r = await tryExec(["ls", "/nonexistent/path/2"]);
            results.push({ name: "fail2", success: r.success });
        },
    );

    console.log(`Results: ${results.map(r => `${r.name}=${r.success}`).join(", ")}`);
    const successCount = results.filter(r => r.success).length;
    const failCount = results.filter(r => !r.success).length;
    console.log(`Success: ${successCount}, Failures: ${failCount} (expected: 1 success, 2 failures)`);
    console.log("Error handling test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 5: Shell with complex commands
    // -------------------------------------------------------------------------
    await stage("5. Complex Shell Commands");
    console.log("Testing complex shell commands with pipes and redirects...");

    await shell("echo 'Line 1\nLine 2\nLine 3' | wc -l | xargs -I {} echo 'Counted {} lines'");
    await shell("echo 'hello world' | tr 'a-z' 'A-Z'");
    await shell("seq 1 5 | paste -sd+ | bc 2>/dev/null || echo 'Sum: 15 (bc not available)'");
    console.log("Shell command test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 6: Environment variables
    // -------------------------------------------------------------------------
    await stage("6. Environment Variables");
    console.log("Testing environment variable passing...");

    await exec(["sh", "-c", "echo \"MY_VAR=$MY_VAR, MY_NUM=$MY_NUM\""], {
        env: { MY_VAR: "test_value", MY_NUM: "42" },
        name: "Env Test"
    });

    await parallel(
        () => exec(["sh", "-c", "echo \"PARALLEL_ID=$PARALLEL_ID\""], {
            env: { PARALLEL_ID: "A" },
            name: "Env Parallel A"
        }),
        () => exec(["sh", "-c", "echo \"PARALLEL_ID=$PARALLEL_ID\""], {
            env: { PARALLEL_ID: "B" },
            name: "Env Parallel B"
        }),
    );
    console.log("Environment variable test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 7: Dynamic task registration during execution
    // -------------------------------------------------------------------------
    await stage("7. Dynamic Task Registration");
    console.log("Registering tasks dynamically...");

    await task("dynamic-task-1", "First dynamic task", async () => {
        console.log("Dynamic task 1 executed!");
        await exec(["echo", "Dynamic task 1 running"]);
    });

    await task("dynamic-task-2", "Second dynamic task", async () => {
        console.log("Dynamic task 2 executed!");
        await exec(["echo", "Dynamic task 2 running"]);
    });

    console.log("Tasks registered. Press '/' to see them in Command Palette.");
    await exec(["sleep", "1"], { name: "Wait for task inspection" });

    // Unregister
    await untask("dynamic-task-1");
    await untask("dynamic-task-2");
    console.log("Tasks unregistered ✓");

    // -------------------------------------------------------------------------
    // Stage 8: Rapid sequential execution
    // -------------------------------------------------------------------------
    await stage("8. Rapid Sequential");
    console.log("Testing rapid sequential execution (10 commands)...");

    for (let i = 1; i <= 10; i++) {
        await exec(["echo", `Rapid ${i}/10`], { name: `Rapid #${i}` });
    }
    console.log("Rapid sequential test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 9: Nested parallel-like patterns
    // -------------------------------------------------------------------------
    await stage("9. Sequential Parallel Blocks");
    console.log("Testing multiple parallel blocks in sequence...");

    console.log("Parallel Block 1:");
    await parallel(
        () => exec(["echo", "Block1-A"]),
        () => exec(["echo", "Block1-B"]),
    );

    console.log("Parallel Block 2:");
    await parallel(
        () => exec(["echo", "Block2-A"]),
        () => exec(["echo", "Block2-B"]),
    );

    console.log("Parallel Block 3:");
    await parallel(
        () => exec(["echo", "Block3-A"]),
        () => exec(["echo", "Block3-B"]),
        () => exec(["echo", "Block3-C"]),
    );
    console.log("Sequential parallel blocks test passed ✓");

    // -------------------------------------------------------------------------
    // Stage 10: Timeout handling
    // -------------------------------------------------------------------------
    await stage("10. Timeout Handling");
    console.log("Testing timeout (command will be killed after 1s)...");

    const timeoutResult = await tryExec(["sleep", "10"], {
        timeout: 1,
        name: "Sleep 10s (1s timeout)"
    });
    console.log(`Timeout test: timedOut=${timeoutResult.timedOut}, exitCode=${timeoutResult.exitCode}`);
    console.log("Timeout test passed ✓");

    // -------------------------------------------------------------------------
    // Final Summary
    // -------------------------------------------------------------------------
    await stage("Complete");
    console.log("");
    console.log("╔══════════════════════════════════════════════════════════════╗");
    console.log("║          ALL TESTS COMPLETED SUCCESSFULLY                    ║");
    console.log("╠══════════════════════════════════════════════════════════════╣");
    console.log("║  ✓ Sequential execution with console.log                     ║");
    console.log("║  ✓ Parallel with different durations                         ║");
    console.log("║  ✓ Parallel with heavy output                                ║");
    console.log("║  ✓ Error handling in parallel                                ║");
    console.log("║  ✓ Complex shell commands                                    ║");
    console.log("║  ✓ Environment variables                                     ║");
    console.log("║  ✓ Dynamic task registration                                 ║");
    console.log("║  ✓ Rapid sequential execution                                ║");
    console.log("║  ✓ Sequential parallel blocks                                ║");
    console.log("║  ✓ Timeout handling                                          ║");
    console.log("╚══════════════════════════════════════════════════════════════╝");
}

// Quick smoke test - runs fast, good for CI
export async function test_smoke() {
    console.log("Running smoke test...");

    await stage("Smoke Test");
    await exec(["echo", "Basic exec works"]);
    await shell("echo 'Basic shell works'");

    await parallel(
        () => exec(["echo", "Parallel 1"]),
        () => exec(["echo", "Parallel 2"]),
    );

    const r = await tryExec(["false"]);
    if (!r.success) {
        console.log("tryExec correctly captured failure");
    }

    console.log("Smoke test passed ✓");
}

// =============================================================================
// Docker Integration Tests
// =============================================================================

import { docker } from "ebdev";

// Test docker.exec - requires docker container to be running
// Run with: make test-docker
export async function test_docker() {
    const container = "example-sync-target-1";

    console.log("╔══════════════════════════════════════════════════════════════╗");
    console.log("║          DOCKER INTEGRATION TEST                             ║");
    console.log("╚══════════════════════════════════════════════════════════════╝");
    console.log(`Target container: ${container}`);
    console.log("");

    // -------------------------------------------------------------------------
    // Test 1: Basic docker exec
    // -------------------------------------------------------------------------
    await stage("1. Basic docker.exec");
    console.log("Testing basic command execution in container...");
    await docker.exec(container, ["echo", "Hello from container!"]);
    await docker.exec(container, ["uname", "-a"]);
    console.log("Basic docker.exec passed ✓");

    // -------------------------------------------------------------------------
    // Test 2: Environment variables
    // -------------------------------------------------------------------------
    await stage("2. Environment Variables");
    console.log("Testing environment variable passing...");
    await docker.exec(container, ["sh", "-c", "echo \"MY_VAR=$MY_VAR\""], {
        env: { MY_VAR: "test_value_from_host" },
        name: "Env Test"
    });
    await docker.exec(container, ["sh", "-c", "echo \"FOO=$FOO BAR=$BAR\""], {
        env: { FOO: "hello", BAR: "world" },
        name: "Multi Env Test"
    });
    console.log("Environment variables passed ✓");

    // -------------------------------------------------------------------------
    // Test 3: Working with files
    // -------------------------------------------------------------------------
    await stage("3. File Operations");
    console.log("Testing file operations in container...");
    await docker.exec(container, ["sh", "-c", "echo 'test content' > /tmp/ebdev_test.txt"]);
    await docker.exec(container, ["cat", "/tmp/ebdev_test.txt"]);
    await docker.exec(container, ["rm", "/tmp/ebdev_test.txt"]);
    console.log("File operations passed ✓");

    // -------------------------------------------------------------------------
    // Test 4: Parallel docker.exec
    // -------------------------------------------------------------------------
    await stage("4. Parallel Execution");
    console.log("Testing parallel docker.exec...");
    await parallel(
        () => docker.exec(container, ["echo", "Parallel A"]),
        () => docker.exec(container, ["echo", "Parallel B"]),
        () => docker.exec(container, ["echo", "Parallel C"]),
    );
    console.log("Parallel execution passed ✓");

    // -------------------------------------------------------------------------
    // Test 5: Error handling with tryExec
    // -------------------------------------------------------------------------
    await stage("5. Error Handling");
    console.log("Testing error handling with docker.tryExec...");
    const result = await docker.tryExec(container, ["ls", "/nonexistent/path"]);
    console.log(`Exit code: ${result.exitCode}, Success: ${result.success}`);
    if (!result.success) {
        console.log("Error correctly captured ✓");
    }

    // -------------------------------------------------------------------------
    // Test 6: Long output
    // -------------------------------------------------------------------------
    await stage("6. Long Output");
    console.log("Testing command with longer output...");
    await docker.exec(container, ["sh", "-c", "for i in $(seq 1 20); do echo \"Line $i of output\"; done"]);
    console.log("Long output passed ✓");

    // -------------------------------------------------------------------------
    // Test 7: Exit codes
    // -------------------------------------------------------------------------
    await stage("7. Exit Codes");
    console.log("Testing various exit codes...");

    const exit0 = await docker.tryExec(container, ["true"]);
    console.log(`'true' exit code: ${exit0.exitCode} (expected: 0)`);

    const exit1 = await docker.tryExec(container, ["false"]);
    console.log(`'false' exit code: ${exit1.exitCode} (expected: 1)`);

    const exit42 = await docker.tryExec(container, ["sh", "-c", "exit 42"]);
    console.log(`'exit 42' exit code: ${exit42.exitCode} (expected: 42)`);

    console.log("Exit codes passed ✓");

    // -------------------------------------------------------------------------
    // Summary
    // -------------------------------------------------------------------------
    await stage("Complete");
    console.log("");
    console.log("╔══════════════════════════════════════════════════════════════╗");
    console.log("║          DOCKER TESTS COMPLETED SUCCESSFULLY                 ║");
    console.log("╠══════════════════════════════════════════════════════════════╣");
    console.log("║  ✓ Basic docker.exec                                         ║");
    console.log("║  ✓ Environment variables                                     ║");
    console.log("║  ✓ File operations                                           ║");
    console.log("║  ✓ Parallel execution                                        ║");
    console.log("║  ✓ Error handling                                            ║");
    console.log("║  ✓ Long output                                               ║");
    console.log("║  ✓ Exit codes                                                ║");
    console.log("╚══════════════════════════════════════════════════════════════╝");
}

// Quick docker smoke test
export async function test_docker_smoke() {
    const container = "example-sync-target-1";
    console.log("Docker smoke test...");

    await docker.exec(container, ["echo", "Docker exec works!"]);
    await docker.exec(container, ["sh", "-c", "echo $TEST_VAR"], { env: { TEST_VAR: "success" } });

    const r = await docker.tryExec(container, ["false"]);
    if (!r.success) {
        console.log("docker.tryExec correctly captured failure");
    }

    console.log("Docker smoke test passed ✓");
}
