use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::{extract_tar_gz, extract_zip};
use crate::pinned::{install_dir_name, ArchiveKind, Asset, NETCOREDBG_VERSION};
use crate::verify::verify_sha256;

fn partial_dir_name() -> String {
    format!(".partial-{NETCOREDBG_VERSION}")
}

/// Cheap "is it already installed" check: one `stat`, no network, no hashing.
/// A complete install directory only ever appears via an atomic rename (see `install`).
pub fn find_installed(root: &Path, asset: &Asset) -> Option<PathBuf> {
    let exe = root.join(install_dir_name()).join(asset.executable);
    exe.is_file().then_some(exe)
}

/// Verifies, extracts and atomically publishes an install. `mark_executable` is
/// supplied by the host (WASI cannot chmod).
pub fn install(
    root: &Path,
    asset: &Asset,
    archive: &[u8],
    mark_executable: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    // Verify before extracting a single byte: the checksum is pinned in this extension.
    verify_sha256(archive, asset.sha256).map_err(|e| format!("{}: {e}", asset.name))?;

    let partial = root.join(partial_dir_name());
    let final_dir = root.join(install_dir_name());
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|e| format!("create {}: {e}", partial.display()))?;

    let result = (|| {
        match asset.kind {
            ArchiveKind::TarGz => extract_tar_gz(archive, &partial)?,
            ArchiveKind::Zip => extract_zip(archive, &partial)?,
        }
        let exe = partial.join(asset.executable);
        if !exe.is_file() {
            return Err(format!("{} not found in {}", asset.executable, asset.name));
        }
        mark_executable(&exe)?;
        // Publishing by rename means a crash mid-install can never leave a half-written
        // directory that `find_installed` would mistake for a working one.
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&partial, &final_dir)
            .map_err(|e| format!("publish {}: {e}", final_dir.display()))
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&partial);
        return Err(e);
    }
    Ok(final_dir.join(asset.executable))
}

/// Best effort: drop installs of other versions and abandoned partial extractions.
pub fn prune_stale(root: &Path) {
    let keep = install_dir_name();
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale =
            (name.starts_with("netcoredbg-") && name != keep) || name.starts_with(".partial-");
        if stale && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pinned::{ArchiveKind, Asset};
    use crate::testutil::{scratch, tar_gz};
    use crate::verify::sha256_hex;
    use std::cell::Cell;

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    fn asset_for(archive: &[u8]) -> Asset {
        Asset {
            name: "test.tar.gz",
            sha256: leak(sha256_hex(archive)),
            kind: ArchiveKind::TarGz,
            executable: "netcoredbg/netcoredbg",
        }
    }

    #[test]
    fn installs_atomically_and_is_then_found() {
        let root = scratch("install-ok");
        let archive = tar_gz(&[("netcoredbg/netcoredbg", b"bin")]);
        let asset = asset_for(&archive);
        assert!(find_installed(&root, &asset).is_none());
        let marked = Cell::new(false);
        let exe = install(&root, &asset, &archive, &|_| {
            marked.set(true);
            Ok(())
        })
        .unwrap();
        assert!(marked.get());
        assert_eq!(fs::read(&exe).unwrap(), b"bin");
        assert_eq!(find_installed(&root, &asset), Some(exe));
        assert!(!root.join(partial_dir_name()).exists());
    }

    #[test]
    fn tampered_archive_installs_nothing() {
        let root = scratch("install-tampered");
        let archive = tar_gz(&[("netcoredbg/netcoredbg", b"bin")]);
        let mut asset = asset_for(&archive);
        asset.sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let err = install(&root, &asset, &archive, &|_| Ok(())).unwrap_err();
        assert!(err.contains("checksum mismatch"));
        assert!(find_installed(&root, &asset).is_none());
        assert!(!root.join(partial_dir_name()).exists());
    }

    #[test]
    fn missing_executable_cleans_up() {
        let root = scratch("install-noexe");
        let archive = tar_gz(&[("netcoredbg/other", b"x")]);
        let asset = asset_for(&archive);
        assert!(install(&root, &asset, &archive, &|_| Ok(())).is_err());
        assert!(!root.join(partial_dir_name()).exists());
        assert!(!root.join(install_dir_name()).exists());
    }

    #[test]
    fn prune_removes_other_versions_only() {
        let root = scratch("prune");
        for d in [
            "netcoredbg-1.0.0",
            ".partial-9",
            &install_dir_name(),
            "unrelated",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        prune_stale(&root);
        assert!(!root.join("netcoredbg-1.0.0").exists());
        assert!(!root.join(".partial-9").exists());
        assert!(root.join(install_dir_name()).exists());
        assert!(root.join("unrelated").exists());
    }
}
