//! Minimale Bridge-Binary für Remote-Command-Execution
//! Diese Binary wird in Container kopiert und führt Befehle aus.

fn main() {
    if let Err(e) = ebdev_remote::run_bridge() {
        eprintln!("bridge error: {}", e);
        std::process::exit(1);
    }
}
