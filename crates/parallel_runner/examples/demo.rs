use ebdev_parallel_runner::{run_parallel, CommandDef, Layout};

fn main() {
    let commands = vec![
        CommandDef::new("watch-1", "bash")
            .args(vec!["-c".to_string(), "while true; do date; sleep 1; done".to_string()]),
        CommandDef::new("watch-2", "bash")
            .args(vec!["-c".to_string(), "for i in $(seq 1 100); do echo \"Line $i\"; sleep 0.2; done".to_string()]),
        CommandDef::new("htop-like", "bash")
            .args(vec!["-c".to_string(), "top -l 0 -s 1 2>/dev/null || top -b -d 1 2>/dev/null || while true; do ps aux | head -20; sleep 1; clear; done".to_string()]),
    ];

    // Use Stacked layout - commands untereinander, full width
    if let Err(e) = run_parallel(commands, Layout::Stacked) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
