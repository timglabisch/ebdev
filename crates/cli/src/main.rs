use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("ebdev v{}", ebdev_toolchain::version());

    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 && args[1] == "node" && args.len() > 2 && args[2] == "install" {
        let version = args.get(3).map(|s| s.as_str()).unwrap_or("22.12.0");
        let base_path = PathBuf::from(".");
        ebdev_toolchain_node::install_node(version, &base_path).await?;
    }

    Ok(())
}
