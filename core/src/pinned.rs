//! netcoredbg releases this extension installs by default.
//!
//! The SHA-256 values are baked in at release time so an install never trusts a
//! network-supplied checksum: a tampered or substituted archive cannot pass.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    X86,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: &'static str,
    pub sha256: &'static str,
    pub kind: ArchiveKind,
    /// Path of the debugger executable inside the extracted tree.
    pub executable: &'static str,
}

pub const NETCOREDBG_VERSION: &str = "3.2.0-1092";

const BASE_URL: &str = "https://github.com/Samsung/netcoredbg/releases/download";

static LINUX_X64: Asset = Asset {
    name: "netcoredbg-linux-amd64.tar.gz",
    sha256: "080eb3b2d2152465f599d3b33d1ee6e747794e11cc0a3773ec689f5e5f2c5afa",
    kind: ArchiveKind::TarGz,
    executable: "netcoredbg/netcoredbg",
};
static LINUX_ARM64: Asset = Asset {
    name: "netcoredbg-linux-arm64.tar.gz",
    sha256: "065ff49badec8a695dbea2de6ab6a330c774a191e426a217ab8cc05250627ccb",
    kind: ArchiveKind::TarGz,
    executable: "netcoredbg/netcoredbg",
};
static MAC_ARM64: Asset = Asset {
    name: "netcoredbg-osx-arm64.zip",
    sha256: "f4fa33b3ff874910cc184b4bb3b9c56d0abdf5c6521cee0b144d7c6e4a6e59ea",
    kind: ArchiveKind::Zip,
    executable: "netcoredbg/netcoredbg",
};
static WIN_X64: Asset = Asset {
    name: "netcoredbg-win64.zip",
    sha256: "3c410a45fa502415203a94fcb88654af65bf8e3dac158a5527a722e7a6b9274a",
    kind: ArchiveKind::Zip,
    executable: "netcoredbg/netcoredbg.exe",
};

pub fn asset_for(os: Os, arch: Arch) -> Result<&'static Asset, String> {
    match (os, arch) {
        (Os::Linux, Arch::X86_64) => Ok(&LINUX_X64),
        (Os::Linux, Arch::Aarch64) => Ok(&LINUX_ARM64),
        (Os::Mac, Arch::Aarch64) => Ok(&MAC_ARM64),
        (Os::Windows, Arch::X86_64) => Ok(&WIN_X64),
        _ => Err(format!(
            "netcoredbg {NETCOREDBG_VERSION} has no prebuilt binary for {os:?}/{arch:?}. \
             Install netcoredbg yourself and either put it on PATH or set \
             `\"dap\": {{ \"netcoredbg\": {{ \"binary\": \"/path/to/netcoredbg\" }} }}` in Zed settings."
        )),
    }
}

pub fn download_url(asset: &Asset) -> String {
    format!("{BASE_URL}/{NETCOREDBG_VERSION}/{}", asset.name)
}

/// Directory name (inside the extension work dir) holding one complete install.
pub fn install_dir_name() -> String {
    format!("netcoredbg-{NETCOREDBG_VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_platform_has_a_well_formed_asset() {
        for (os, arch) in [
            (Os::Linux, Arch::X86_64),
            (Os::Linux, Arch::Aarch64),
            (Os::Mac, Arch::Aarch64),
            (Os::Windows, Arch::X86_64),
        ] {
            let a = asset_for(os, arch).unwrap();
            assert_eq!(a.sha256.len(), 64);
            assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(download_url(a).starts_with("https://github.com/Samsung/netcoredbg/"));
        }
    }

    #[test]
    fn unsupported_platform_explains_the_manual_route() {
        let err = asset_for(Os::Mac, Arch::X86_64).unwrap_err();
        assert!(err.contains("binary"));
        assert!(asset_for(Os::Windows, Arch::Aarch64).is_err());
    }
}
