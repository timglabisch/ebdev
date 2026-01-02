#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("ebdev v{}", ebdev_toolchain::version());
    Ok(())
}
