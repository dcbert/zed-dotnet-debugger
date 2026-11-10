//! Extraction of the netcoredbg archives, done in-process so the archive can be
//! checksummed *before* any byte of it is written to disk as an executable.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

// Hard caps so a corrupt or hostile archive (zip bomb, entry flood) cannot
// exhaust the user's disk or memory even if it somehow passed verification.
const MAX_ENTRIES: usize = 2048;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Turns an archive entry name into a path that is guaranteed to stay inside
/// the destination directory. `Ok(None)` means "skip this entry".
pub fn safe_relative_path(raw: &str) -> Result<Option<PathBuf>, String> {
    // Reject at the boundary: absolute paths, `..`, drive letters / NTFS
    // streams (`:`) and NUL would let an entry write outside the install dir.
    if raw.contains('\0') {
        return Err(format!("archive entry {raw:?} contains a NUL byte"));
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(format!("archive entry {raw:?} is an absolute path"));
    }
    let mut out = PathBuf::new();
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(format!("archive entry {raw:?} escapes the install directory")),
            c if c.contains(':') => {
                return Err(format!("archive entry {raw:?} contains a forbidden character"))
            }
            c => {
                if out.as_os_str().is_empty() && c == "__MACOSX" {
                    return Ok(None);
                }
                out.push(c);
            }
        }
    }
    if out.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

struct Budget {
    entries: usize,
    total: u64,
}

impl Budget {
    fn new() -> Self {
        Self { entries: 0, total: 0 }
    }

    fn entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(format!("archive has more than {MAX_ENTRIES} entries"));
        }
        Ok(())
    }
}

fn write_file(reader: &mut dyn Read, target: &Path, budget: &mut Budget) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut file = fs::File::create(target).map_err(|e| format!("create {}: {e}", target.display()))?;
    // Bound the copy by the cap, not by the size the header claims: headers can lie.
    let written = std::io::copy(&mut reader.take(MAX_FILE_BYTES + 1), &mut file)
        .map_err(|e| format!("write {}: {e}", target.display()))?;
    if written > MAX_FILE_BYTES {
        return Err(format!("archive entry {} exceeds {MAX_FILE_BYTES} bytes", target.display()));
    }
    budget.total += written;
    if budget.total > MAX_TOTAL_BYTES {
        return Err(format!("archive expands beyond {MAX_TOTAL_BYTES} bytes"));
    }
    Ok(())
}

pub fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    let mut budget = Budget::new();
    for entry in archive.entries().map_err(|e| format!("read tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read tar entry: {e}"))?;
        budget.entry()?;
        let raw = entry
            .path()
            .map_err(|e| format!("tar entry path: {e}"))?
            .to_string_lossy()
            .into_owned();
        let Some(rel) = safe_relative_path(&raw)? else { continue };
        let target = dest.join(rel);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(|e| format!("create {}: {e}", target.display()))?;
        } else if kind.is_file() {
            write_file(&mut entry, &target, &mut budget)?;
        } else if kind.is_pax_global_extensions() {
            continue;
        } else {
            // Links could point outside `dest` and be followed by a later entry.
            return Err(format!("archive entry {raw:?} has unsupported type {kind:?}"));
        }
    }
    Ok(())
}

pub fn extract_zip(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| format!("read zip: {e}"))?;
    let mut budget = Budget::new();
    for index in 0..archive.len() {
        budget.entry()?;
        let mut file = archive.by_index(index).map_err(|e| format!("read zip entry: {e}"))?;
        let raw = file
            .name()
            .map_err(|e| format!("zip entry name: {e}"))?
            .into_owned();
        let Some(rel) = safe_relative_path(&raw)? else { continue };
        let target = dest.join(rel);
        if file.is_symlink() {
            return Err(format!("archive entry {raw:?} is a symlink"));
        }
        if file.is_dir() {
            fs::create_dir_all(&target).map_err(|e| format!("create {}: {e}", target.display()))?;
        } else {
            write_file(&mut file, &target, &mut budget)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{scratch, tar_gz, zip_archive};

    #[test]
    fn rejects_escaping_and_absolute_names() {
        for bad in ["../evil", "a/../../evil", "/etc/passwd", "C:/x", "a/b:stream", "a\\..\\..\\x", "nul\0"] {
            assert!(safe_relative_path(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn accepts_normal_names_and_skips_resource_forks() {
        assert_eq!(
            safe_relative_path("netcoredbg/./libdbgshim.so").unwrap(),
            Some(PathBuf::from("netcoredbg/libdbgshim.so"))
        );
        assert_eq!(safe_relative_path("__MACOSX/netcoredbg/._x").unwrap(), None);
        assert_eq!(safe_relative_path("./").unwrap(), None);
    }

    #[test]
    fn extracts_tar_gz() {
        let dest = scratch("tar-ok");
        let data = tar_gz(&[("netcoredbg/netcoredbg", b"bin"), ("netcoredbg/lib.so", b"lib")]);
        extract_tar_gz(&data, &dest).unwrap();
        assert_eq!(fs::read(dest.join("netcoredbg/netcoredbg")).unwrap(), b"bin");
        assert_eq!(fs::read(dest.join("netcoredbg/lib.so")).unwrap(), b"lib");
    }

    #[test]
    fn tar_traversal_is_rejected_and_nothing_escapes() {
        let dest = scratch("tar-evil");
        let data = tar_gz(&[("../escaped.txt", b"x")]);
        assert!(extract_tar_gz(&data, &dest).unwrap_err().contains("escapes"));
        assert!(!dest.parent().unwrap().join("escaped.txt").exists());
    }

    #[test]
    fn extracts_zip_and_skips_macosx_forks() {
        let dest = scratch("zip-ok");
        let data = zip_archive(&[
            ("netcoredbg/netcoredbg.exe", b"exe"),
            ("__MACOSX/netcoredbg/._netcoredbg.exe", b"fork"),
        ]);
        extract_zip(&data, &dest).unwrap();
        assert_eq!(fs::read(dest.join("netcoredbg/netcoredbg.exe")).unwrap(), b"exe");
        assert!(!dest.join("__MACOSX").exists());
    }

    #[test]
    fn zip_traversal_is_rejected() {
        let dest = scratch("zip-evil");
        let data = zip_archive(&[("a/../../escaped.txt", b"x")]);
        assert!(extract_zip(&data, &dest).is_err());
    }

    #[test]
    fn garbage_input_is_an_error_not_a_panic() {
        let dest = scratch("garbage");
        assert!(extract_tar_gz(b"not a tarball", &dest).is_err());
        assert!(extract_zip(b"not a zip", &dest).is_err());
    }
}
