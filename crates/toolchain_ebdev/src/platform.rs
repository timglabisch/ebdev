#[derive(Debug, Clone, Copy)]
pub enum Platform {
    Macos,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Platform::Macos;

        #[cfg(target_os = "linux")]
        return Platform::Linux;

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        compile_error!("Unsupported platform");
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Linux => "linux",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Self {
        #[cfg(all(target_pointer_width = "64", any(target_arch = "arm", target_arch = "aarch64")))]
        return Arch::Aarch64;

        #[cfg(all(target_pointer_width = "64", not(any(target_arch = "arm", target_arch = "aarch64"))))]
        return Arch::X86_64;

        #[cfg(not(target_pointer_width = "64"))]
        compile_error!("Only 64-bit architectures are supported");
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}
