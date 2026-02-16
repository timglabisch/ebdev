// Simple WASM hello world - demonstrates basic WASI stdout
fn main() {
    println!("Hello from WASM!");
    println!("Args: {:?}", std::env::args().skip(1).collect::<Vec<_>>());

    for (key, value) in std::env::vars() {
        if key.starts_with("MY_") || key.starts_with("APP_") {
            println!("  {}={}", key, value);
        }
    }
}
