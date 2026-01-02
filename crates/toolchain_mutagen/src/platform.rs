#[derive(Debug, Clone, Copy)]
pub enum Platform {
    Darwin,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Platform::Darwin;

        #[cfg(target_os = "linux")]
        return Platform::Linux;

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        compile_error!("Unsupported platform");
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Darwin => "darwin",
            Platform::Linux => "linux",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Arch {
    Amd64,
    Arm64,
}

impl Arch {
    pub fn current() -> Self {
        #[cfg(all(target_pointer_width = "64", any(target_arch = "arm", target_arch = "aarch64")))]
        return Arch::Arm64;

        #[cfg(all(target_pointer_width = "64", not(any(target_arch = "arm", target_arch = "aarch64"))))]
        return Arch::Amd64;

        #[cfg(not(target_pointer_width = "64"))]
        compile_error!("Only 64-bit architectures are supported");
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}
