use std::io::{Cursor, Write};
use std::path::PathBuf;

/// Fresh, empty directory per test. Under the WASI runner `DD_SCRATCH` is a preopened host temp dir.
pub fn scratch(name: &str) -> PathBuf {
    let base = std::env::var("DD_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join(format!("dd-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        // Written raw: `set_path` itself refuses `..`, which is exactly what we want to test against.
        let bytes = header.as_old_mut();
        bytes.name = [0; 100];
        bytes.name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        builder.append(&header, *body).unwrap();
    }
    let tar_bytes = builder.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&tar_bytes).unwrap();
    encoder.finish().unwrap()
}

pub fn zip_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, body) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
