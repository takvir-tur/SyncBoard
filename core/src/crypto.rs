use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

const CONFIG_FILE_NAME: &str = "config.bin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    EncryptionFailed,
    DecryptionFailed,
    InvalidPairingUri(&'static str),
    Io(String),
    Serialization(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EncryptionFailed => write!(f, "AEAD encryption failed"),
            Self::DecryptionFailed => write!(f, "AEAD decryption failed"),
            Self::InvalidPairingUri(reason) => write!(f, "invalid pairing URI: {reason}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
        }
    }
}

impl std::error::Error for CryptoError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub cluster_key: [u8; 32],
    pub relay_url: String,
}

impl ClusterConfig {
    pub fn generate(relay_url: &str) -> Self {
        Self {
            cluster_key: generate_cluster_key(),
            relay_url: normalize_relay_url(relay_url),
        }
    }

    pub fn room_id(&self) -> [u8; 32] {
        derive_room_id(&self.cluster_key)
    }

    pub fn room_id_hex(&self) -> String {
        hex::encode(self.room_id())
    }

    pub fn encryption_key(&self) -> [u8; 32] {
        derive_encryption_key(&self.cluster_key)
    }

    pub fn pairing_uri(&self) -> String {
        format_pairing_uri(&self.cluster_key, &self.relay_url)
    }
}

pub fn generate_cluster_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn derive_room_id(cluster_key: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("room", cluster_key)
}

pub fn derive_encryption_key(cluster_key: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("e2ee", cluster_key)
}

pub fn random_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

pub fn random_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    OsRng.fill_bytes(&mut id);
    id
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn encrypt_aead(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::EncryptionFailed)
}

pub fn decrypt_aead(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| CryptoError::DecryptionFailed)
}

pub fn normalize_relay_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    }
}

pub fn format_pairing_uri(cluster_key: &[u8; 32], relay_url: &str) -> String {
    let k_hex = hex::encode(cluster_key);
    let r_enc = percent_encode(&normalize_relay_url(relay_url));
    format!("clipsync://pair?k={k_hex}&r={r_enc}")
}

pub fn parse_pairing_uri(uri: &str) -> Result<([u8; 32], String), CryptoError> {
    let trimmed = uri.trim();
    let query = trimmed
        .strip_prefix("clipsync://pair?")
        .ok_or(CryptoError::InvalidPairingUri("missing clipsync://pair? prefix"))?;

    let mut key_hex: Option<String> = None;
    let mut relay_raw: Option<String> = None;

    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "k" => key_hex = Some(percent_decode(v)?),
                "r" => relay_raw = Some(percent_decode(v)?),
                _ => {}
            }
        }
    }

    let key_hex = key_hex.ok_or(CryptoError::InvalidPairingUri("missing k parameter"))?;
    let key_bytes = hex::decode(key_hex.trim())
        .map_err(|_| CryptoError::InvalidPairingUri("k is not valid hex"))?;
    if key_bytes.len() != 32 {
        return Err(CryptoError::InvalidPairingUri("k must be 32 bytes (64 hex chars)"));
    }
    let mut cluster_key = [0u8; 32];
    cluster_key.copy_from_slice(&key_bytes);

    let relay_raw = relay_raw.ok_or(CryptoError::InvalidPairingUri("missing r parameter"))?;
    let relay_trimmed = relay_raw.trim().trim_end_matches('/');
    let rest = relay_trimmed
        .strip_prefix("wss://")
        .or_else(|| relay_trimmed.strip_prefix("ws://"))
        .ok_or(CryptoError::InvalidPairingUri("relay URL must use ws:// or wss://"))?;
    if rest.is_empty() {
        return Err(CryptoError::InvalidPairingUri("relay URL host is empty"));
    }

    Ok((cluster_key, relay_trimmed.to_string()))
}

pub fn load_or_create_config(
    data_dir: &Path,
    default_relay_url: &str,
) -> Result<ClusterConfig, CryptoError> {
    fs::create_dir_all(data_dir).map_err(|e| CryptoError::Io(e.to_string()))?;
    let path = data_dir.join(CONFIG_FILE_NAME);
    if path.exists() {
        let bytes = fs::read(&path).map_err(|e| CryptoError::Io(e.to_string()))?;
        let mut cfg: ClusterConfig =
            bincode::deserialize(&bytes).map_err(|e| CryptoError::Serialization(e.to_string()))?;
        if cfg.relay_url.trim().is_empty() {
            cfg.relay_url = normalize_relay_url(default_relay_url);
        }
        return Ok(cfg);
    }

    let cfg = ClusterConfig::generate(default_relay_url);
    save_config(data_dir, &cfg)?;
    Ok(cfg)
}

pub fn save_config(data_dir: &Path, cfg: &ClusterConfig) -> Result<(), CryptoError> {
    fs::create_dir_all(data_dir).map_err(|e| CryptoError::Io(e.to_string()))?;
    let encoded = bincode::serialize(cfg).map_err(|e| CryptoError::Serialization(e.to_string()))?;
    atomic_write(&data_dir.join(CONFIG_FILE_NAME), &encoded)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CryptoError> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("data");
    let tmp_path = path.with_file_name(format!("{file_name}.tmp"));

    {
        let mut file = fs::File::create(&tmp_path).map_err(|e| CryptoError::Io(e.to_string()))?;
        file.write_all(bytes).map_err(|e| CryptoError::Io(e.to_string()))?;
        file.sync_all().map_err(|e| CryptoError::Io(e.to_string()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&tmp_path, perms);
    }

    fs::rename(&tmp_path, path).map_err(|e| CryptoError::Io(e.to_string()))?;
    Ok(())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_nibble(b >> 4));
            out.push(hex_nibble(b & 0x0f));
        }
    }
    out
}

fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

fn percent_decode(input: &str) -> Result<String, CryptoError> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(CryptoError::InvalidPairingUri("truncated percent escape"));
                }
                let hi = from_hex_digit(bytes[i + 1])?;
                let lo = from_hex_digit(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| CryptoError::InvalidPairingUri("non-UTF-8 parameter"))
}

fn from_hex_digit(b: u8) -> Result<u8, CryptoError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(CryptoError::InvalidPairingUri("invalid percent escape hex")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_derivation_is_deterministic_and_domain_separated() {
        let key = [0x42u8; 32];
        let room1 = derive_room_id(&key);
        let room2 = derive_room_id(&key);
        let e2ee = derive_encryption_key(&key);

        assert_eq!(room1, room2);
        assert_ne!(room1, e2ee);
        assert_ne!(room1, key);
        assert_ne!(e2ee, key);
    }

    #[test]
    fn aead_roundtrip_and_tamper_detection() {
        let key = derive_encryption_key(&[0x11u8; 32]);
        let nonce = [0x22u8; 12];
        let aad = b"header-v1";
        let plaintext = b"hello secure clipboard";

        let ciphertext = encrypt_aead(&key, &nonce, aad, plaintext).expect("encrypt");
        let decrypted = decrypt_aead(&key, &nonce, aad, &ciphertext).expect("decrypt");
        assert_eq!(decrypted, plaintext);

        let wrong_key = derive_encryption_key(&[0x99u8; 32]);
        assert_eq!(
            decrypt_aead(&wrong_key, &nonce, aad, &ciphertext),
            Err(CryptoError::DecryptionFailed)
        );
        assert_eq!(
            decrypt_aead(&key, &nonce, b"tampered-aad", &ciphertext),
            Err(CryptoError::DecryptionFailed)
        );
    }

    #[test]
    fn pairing_uri_roundtrip_and_validation() {
        let key = [0xABu8; 32];
        let relay = "wss://clipsync-relay.example.workers.dev";
        let uri = format_pairing_uri(&key, relay);
        assert!(uri.starts_with("clipsync://pair?k="));

        let (parsed_key, parsed_relay) = parse_pairing_uri(&uri).expect("valid uri");
        assert_eq!(parsed_key, key);
        assert_eq!(parsed_relay, relay);

        assert!(parse_pairing_uri("https://example.com").is_err());
        assert!(parse_pairing_uri("clipsync://pair?k=deadbeef&r=wss%3A%2F%2Fa.b").is_err());
        assert!(parse_pairing_uri(&format!("clipsync://pair?k={}&r=http://bad", hex::encode(key))).is_err());
    }
}
