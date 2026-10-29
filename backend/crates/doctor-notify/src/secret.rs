//! Notification secrets at rest: AES-256-GCM under one master key. The key
//! comes from `DOCTOR_SECRET_KEY` or a `secret.key` file beside the history
//! database. This keeps secrets out of a copied or backed-up database file;
//! it does not protect against a compromised host.
use std::io::Write;
use std::path::Path;

use doctor_contracts::NotifyKeySource;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

const BLOB_VERSION: u8 = 1;
const AAD_PREFIX: &str = "sr-doctor.notify.v1:";

#[derive(Clone, Eq, PartialEq)]
pub struct SecretKey([u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretKey(..)")
    }
}

#[derive(Debug)]
pub enum KeyError {
    /// `DOCTOR_SECRET_KEY` or the key file is not 64 hex characters.
    Malformed,
    Io(std::io::Error),
    Random,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => {
                formatter.write_str("secret_key_malformed: expected 64 hex characters")
            }
            Self::Io(error) => write!(formatter, "secret_key_file:{}", error.kind()),
            Self::Random => formatter.write_str("secret_key_random_unavailable"),
        }
    }
}

impl std::error::Error for KeyError {}

impl SecretKey {
    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 64 {
            return None;
        }
        let mut key = [0u8; 32];
        for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(chunk).ok()?;
            key[index] = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(key))
    }

    pub fn generate() -> Result<Self, KeyError> {
        let mut key = [0u8; 32];
        SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| KeyError::Random)?;
        Ok(Self(key))
    }

    fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// `DOCTOR_SECRET_KEY` when set; otherwise the key file, created with
    /// owner-only permissions on first use.
    pub fn load_or_create(
        environment: Option<&str>,
        file: &Path,
    ) -> Result<(Self, NotifyKeySource), KeyError> {
        if let Some(value) = environment.filter(|value| !value.trim().is_empty()) {
            return Self::from_hex(value)
                .map(|key| (key, NotifyKeySource::Environment))
                .ok_or(KeyError::Malformed);
        }
        match std::fs::read_to_string(file) {
            Ok(text) => {
                return Self::from_hex(&text)
                    .map(|key| (key, NotifyKeySource::File))
                    .ok_or(KeyError::Malformed);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(KeyError::Io(error));
            }
            Err(_) => {}
        }
        let key = Self::generate()?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(KeyError::Io)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(file) {
            Ok(mut handle) => {
                handle
                    .write_all(format!("{}\n", key.to_hex()).as_bytes())
                    .and_then(|()| handle.sync_all())
                    .map_err(KeyError::Io)?;
                Ok((key, NotifyKeySource::File))
            }
            // Another process created it first: use theirs.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Self::load_or_create(None, file)
            }
            Err(error) => Err(KeyError::Io(error)),
        }
    }

    fn cipher(&self) -> LessSafeKey {
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &self.0).expect("32-byte AES-256 key"))
    }

    /// `version || nonce || ciphertext+tag`, bound to `purpose` so a blob
    /// cannot be moved to another field.
    pub fn seal(&self, purpose: &str, plaintext: &[u8]) -> Option<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).ok()?;
        let mut sealed = plaintext.to_vec();
        let aad = format!("{AAD_PREFIX}{purpose}");
        self.cipher()
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut sealed,
            )
            .ok()?;
        let mut blob = Vec::with_capacity(1 + NONCE_LEN + sealed.len());
        blob.push(BLOB_VERSION);
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&sealed);
        Some(blob)
    }

    /// `None` when the blob is damaged or was sealed under another key.
    pub fn open(&self, purpose: &str, blob: &[u8]) -> Option<Vec<u8>> {
        let (&version, rest) = blob.split_first()?;
        if version != BLOB_VERSION || rest.len() < NONCE_LEN {
            return None;
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        let mut buffer = sealed.to_vec();
        let aad = format!("{AAD_PREFIX}{purpose}");
        let plain = self
            .cipher()
            .open_in_place(
                Nonce::try_assume_unique_for_key(nonce).ok()?,
                Aad::from(aad.as_bytes()),
                &mut buffer,
            )
            .ok()?;
        Some(plain.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n01_sealed_secrets_open_only_with_the_same_key_and_purpose() {
        let key = SecretKey::generate().unwrap();
        let blob = key.seal("channel:email", b"smtp-password").unwrap();
        assert!(!blob.windows(13).any(|window| window == b"smtp-password"));
        assert_eq!(key.open("channel:email", &blob).unwrap(), b"smtp-password");
        assert!(key.open("target:1", &blob).is_none());
        assert!(
            SecretKey::generate()
                .unwrap()
                .open("channel:email", &blob)
                .is_none()
        );
        let mut damaged = blob.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(key.open("channel:email", &damaged).is_none());
        assert!(key.open("channel:email", &[]).is_none());
    }

    #[test]
    fn n01_key_text_is_64_hex_and_never_printed() {
        let key = SecretKey::generate().unwrap();
        assert_eq!(SecretKey::from_hex(&key.to_hex()), Some(key.clone()));
        assert!(SecretKey::from_hex("abc").is_none());
        assert!(SecretKey::from_hex(&"zz".repeat(32)).is_none());
        assert_eq!(format!("{key:?}"), "SecretKey(..)");
    }

    #[test]
    fn n01_key_file_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("srd-key-{}", std::process::id()));
        let file = dir.join("secret.key");
        let _ = std::fs::remove_file(&file);
        let (first, source) = SecretKey::load_or_create(None, &file).unwrap();
        assert_eq!(source, NotifyKeySource::File);
        let (second, _) = SecretKey::load_or_create(Some("  "), &file).unwrap();
        assert_eq!(first, second);
        let environment = "11".repeat(32);
        let (from_env, source) = SecretKey::load_or_create(Some(&environment), &file).unwrap();
        assert_eq!(source, NotifyKeySource::Environment);
        assert_ne!(from_env, first);
        assert!(matches!(
            SecretKey::load_or_create(Some("short"), &file),
            Err(KeyError::Malformed)
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
