use sha2::{Digest, Sha256};

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Refuses to continue unless `data` hashes to `expected_hex`.
pub fn verify_sha256(data: &[u8], expected_hex: &str) -> Result<(), String> {
    let actual = sha256_hex(data);
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch: expected {expected_hex}, downloaded file hashes to {actual}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn mismatch_is_rejected_and_case_is_ignored() {
        let good = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert!(verify_sha256(b"abc", good).is_ok());
        assert!(verify_sha256(b"abd", good).unwrap_err().contains("checksum mismatch"));
    }
}
