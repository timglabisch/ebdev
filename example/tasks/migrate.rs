// Example WASM task demonstrating host function calls
// Uses ebdev-wasm guest library for exec(), log(), args()
//
// Note: This requires the ebdev-wasm crate to be available.
// For a simpler version without the guest library, see hello.rs.

use ebdev_wasm::prelude::*;

fn main() {
    let args = args();
    log("Starting migration task...");

    // Run migration
    exec(&["echo", "Running: php artisan migrate"]);

    // Conditional seeding
    if args.iter().any(|a| a == "--seed") {
        log("Seeding database...");
        exec(&["echo", "Running: php artisan db:seed"]);
    }

    // Try a command that might fail
    let result = try_exec(&["echo", "Clearing cache..."]);
    if result.success {
        log("Cache cleared successfully");
    }

    // Shell command
    let result = try_shell("echo 'All done!' | tr 'a-z' 'A-Z'");
    if result.success {
        log("Migration complete!");
    }
}
