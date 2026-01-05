//! Minimale Bridge-Binary für Remote-Command-Execution
//! Diese Binary wird in Container kopiert und führt Befehle aus.
//! Unterstützt PTY für interaktive Sessions.

#[tokio::main]
async fn main() {
    if let Err(e) = ebdev_remote::run_bridge().await {
        eprintln!("bridge error: {}", e);
        std::process::exit(1);
    }
}
