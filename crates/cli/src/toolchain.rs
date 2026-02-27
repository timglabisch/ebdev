use std::ffi::OsString;
use std::path::PathBuf;
use ebdev_toolchain_binary::BinaryEnv;
use ebdev_toolchain_mutagen::MutagenEnv;
use ebdev_toolchain_node::NodeEnv;
use ebdev_toolchain_rust::RustEnv;

pub fn build_path(node_env: &NodeEnv, pnpm_version: Option<&str>, mutagen_env: Option<&MutagenEnv>, rust_env: Option<&RustEnv>, binary_envs: &[BinaryEnv]) -> OsString {
    let mut paths: Vec<PathBuf> = Vec::new();

    // binary toolchain dirs first
    for env in binary_envs {
        paths.push(env.install_dir().to_path_buf());
    }

    // mutagen bin dir (if configured)
    if let Some(env) = mutagen_env {
        paths.push(env.install_dir().to_path_buf());
    }

    // rust bin dir (if configured)
    if let Some(env) = rust_env {
        paths.push(env.bin_dir());
    }

    // pnpm bin dir (if configured)
    if let Some(v) = pnpm_version {
        paths.push(node_env.pnpm_bin_dir(v));
    }

    // node bin dir
    paths.push(node_env.bin_dir());

    // existing PATH
    if let Some(existing) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&existing) {
            paths.push(path);
        }
    }

    std::env::join_paths(paths).unwrap_or_default()
}

pub async fn ensure_toolchain(
    base_path: &PathBuf,
    tc: &ebdev_config::ToolchainConfig,
) -> anyhow::Result<(NodeEnv, Option<MutagenEnv>, Option<RustEnv>, Vec<BinaryEnv>)> {
    let node_version = &tc.node.version;
    let node_env = match NodeEnv::new(base_path, node_version) {
        Ok(env) => env,
        Err(_) => {
            ebdev_toolchain_node::install_node(node_version, base_path).await?;
            NodeEnv::new(base_path, node_version)?
        }
    };

    if let Some(pnpm) = &tc.pnpm {
        if !node_env.pnpm_bin_dir(&pnpm.version).exists() {
            node_env.install_pnpm(&pnpm.version).await?;
        }
    }

    let mutagen_env = if let Some(mutagen) = &tc.mutagen {
        let env = match MutagenEnv::new(base_path, &mutagen.version) {
            Ok(env) => env,
            Err(_) => {
                ebdev_toolchain_mutagen::install_mutagen(&mutagen.version, base_path).await?;
                MutagenEnv::new(base_path, &mutagen.version)?
            }
        };
        Some(env)
    } else {
        None
    };

    let rust_env = if let Some(rust) = &tc.rust {
        let env = match RustEnv::new(base_path, &rust.version) {
            Ok(env) => env,
            Err(_) => {
                ebdev_toolchain_rust::install_rust(&rust.version, base_path).await?;
                RustEnv::new(base_path, &rust.version)?
            }
        };
        Some(env)
    } else {
        None
    };

    let gh_version = tc.gh.as_ref().map(|g| g.version.as_str());

    // Install gh CLI if configured
    if let Some(gh_v) = gh_version {
        if BinaryEnv::new(base_path, "gh", gh_v).is_err() {
            ebdev_toolchain_binary::install_gh(gh_v, base_path).await?;
        }
    }

    let mut binary_envs = Vec::new();

    // Add gh to binary_envs so it appears in PATH
    if let Some(gh_v) = gh_version {
        binary_envs.push(BinaryEnv::new(base_path, "gh", gh_v)?);
    }

    if let Some(binaries) = &tc.binary {
        for (name, cfg) in binaries {
            let env = match BinaryEnv::new(base_path, name, &cfg.version) {
                Ok(env) => env,
                Err(_) => {
                    ebdev_toolchain_binary::install_binary(
                        &ebdev_toolchain_binary::InstallBinaryOptions {
                            name,
                            version: &cfg.version,
                            url_template: &cfg.url,
                            binary_path: cfg.binary.as_deref(),
                            base_path,
                            gh_version,
                        },
                    ).await?;
                    BinaryEnv::new(base_path, name, &cfg.version)?
                }
            };
            binary_envs.push(env);
        }
    }

    Ok((node_env, mutagen_env, rust_env, binary_envs))
}
