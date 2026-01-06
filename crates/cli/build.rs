use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("ebdev-bridge-linux");

    // Look for the Linux binary
    let source_paths = [
        // From workspace root (normal cargo build)
        "../../target/linux/ebdev-bridge",
        // From crate root
        "target/linux/ebdev-bridge",
    ];

    let mut found = false;
    for source in &source_paths {
        let source_path = Path::new(source);
        if source_path.exists() {
            fs::copy(source_path, &dest_path).expect("Failed to copy Linux binary");
            found = true;
            println!("cargo:rerun-if-changed={}", source);
            break;
        }
    }

    if !found {
        // Create empty placeholder - runtime will fall back to file loading
        fs::write(&dest_path, b"").expect("Failed to create placeholder");
    }

    // Rerun if binary changes
    println!("cargo:rerun-if-changed=../../target/linux/ebdev-bridge");
}
