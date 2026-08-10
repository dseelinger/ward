//! Secrets at rest.
//!
//! A stored key is encrypted with the Windows data protection API under the
//! current user, so it is unreadable to any other account on the machine and to
//! this account on any other machine. Copying the data folder somewhere else
//! therefore prompts for the key again, which is correct behavior and happens
//! to make a clean-install test easy.
//!
//! **No environment variable is ever read.** An environment variable is a
//! plaintext bypass of everything above, and it hides first-install problems by
//! making them work on the one machine that has it set.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};

/// Marks a stored blob so a plaintext file left over from hand-editing is
/// obvious rather than being decoded as ciphertext and failing strangely.
const PREFIX: &str = "DPAPI:";

pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("anthropic.key")
}

/// Encrypts and writes. The file is replaced, never appended to.
pub fn store(path: &Path, secret: &str) -> Result<()> {
    let sealed = protect(secret.as_bytes()).context("could not encrypt the key")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let body = format!("{PREFIX}{}", base64(&sealed));
    std::fs::write(path, body).with_context(|| format!("could not write {}", path.display()))?;

    Ok(())
}

/// Reads and decrypts.
///
/// `Ok(None)` means no key has been stored yet, which is an ordinary first-run
/// state rather than a problem. Anything that is present but unreadable is an
/// error carrying a message a person can act on — never a panic, because the
/// usual cause is a folder copied from another machine and the answer is simply
/// to enter the key again.
pub fn load(path: &Path) -> Result<Option<String>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("could not read {}: {e}", path.display())),
    };

    let raw = raw.trim();

    if raw.is_empty() {
        return Ok(None);
    }

    let encoded = raw.strip_prefix(PREFIX).ok_or_else(|| {
        anyhow!(
            "{} is not an encrypted key file — delete it and enter the key again",
            path.display()
        )
    })?;

    let sealed = unbase64(encoded)
        .ok_or_else(|| anyhow!("the stored key is damaged — enter it again to replace it"))?;

    let plain = unprotect(&sealed).context(
        "the stored key could not be decrypted on this account — enter it again to replace it",
    )?;

    let plain = String::from_utf8(plain).context("the stored key is not text")?;

    Ok(Some(plain))
}

fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    // SAFETY: `input` borrows `plain` for the duration of the call only. On
    // success the API allocates `out`, which is copied and then freed below.
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out,
        );

        if ok == 0 {
            return Err(anyhow!("CryptProtectData failed"));
        }

        let sealed = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as HLOCAL);

        Ok(sealed)
    }
}

fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
    // SAFETY: as `protect`, in the other direction.
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: sealed.len() as u32,
            pbData: sealed.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out,
        );

        if ok == 0 {
            return Err(anyhow!("CryptUnprotectData failed"));
        }

        let plain = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as HLOCAL);

        Ok(plain)
    }
}

// Base64, hand-rolled rather than taking a dependency for thirty lines. The
// blob has to survive being written to a text file and read back; that is the
// whole requirement.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);

        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }

    out
}

fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);

    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = ALPHABET.iter().position(|&a| a == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_byte_value() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(unbase64(&base64(&all)).unwrap(), all);
    }

    #[test]
    fn base64_round_trips_every_padding_case() {
        // Lengths 1, 2 and 3 exercise two-pad, one-pad and no-pad output.
        for len in 1..=9 {
            let bytes: Vec<u8> = (0..len as u8).collect();
            assert_eq!(unbase64(&base64(&bytes)).unwrap(), bytes, "length {len}");
        }
    }

    #[test]
    fn base64_rejects_a_character_outside_the_alphabet() {
        assert!(unbase64("abc$def").is_none());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let path = std::env::temp_dir().join("Ward-test-absent-key-file");
        let _ = std::fs::remove_file(&path);
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn a_file_without_the_marker_is_rejected_by_name() {
        let path = std::env::temp_dir().join("Ward-test-plaintext-key");
        std::fs::write(&path, "sk-not-encrypted").unwrap();

        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("not an encrypted key file"), "got: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_stored_key_comes_back() {
        let path = std::env::temp_dir().join("Ward-test-roundtrip-key");
        let _ = std::fs::remove_file(&path);

        store(&path, "sk-ant-example-0123456789").unwrap();

        // The file on disk must not contain the secret in the clear.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.starts_with(PREFIX));
        assert!(!on_disk.contains("sk-ant-example"));

        assert_eq!(load(&path).unwrap().unwrap(), "sk-ant-example-0123456789");

        let _ = std::fs::remove_file(&path);
    }
}
