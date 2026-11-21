use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto::{atomic_write, decrypt_aead, encrypt_aead, random_nonce, CryptoError};
use crate::protocol::ClipKind;

pub const MAX_PINS: usize = 25;
const MAX_TOMBSTONES: usize = 256;
const PINS_FILE_NAME: &str = "pins.bin";
const PINS_MAGIC: &[u8; 5] = b"CSPN\x01";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinItem {
    pub id: [u8; 16],
    pub kind: ClipKind,
    pub mime: String,
    pub data: Vec<u8>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    Crypto(CryptoError),
    Io(String),
    CorruptStore(&'static str),
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crypto(e) => write!(f, "{e}"),
            Self::Io(msg) => write!(f, "pin store I/O error: {msg}"),
            Self::CorruptStore(reason) => write!(f, "corrupt pins store: {reason}"),
        }
    }
}

impl std::error::Error for PinError {}

impl From<CryptoError> for PinError {
    fn from(err: CryptoError) -> Self {
        Self::Crypto(err)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedPins {
    items: Vec<PinItem>,
    tombstones: HashMap<[u8; 16], u64>,
}

pub struct PinStore {
    path: PathBuf,
    enc_key: [u8; 32],
    items: Vec<PinItem>,
    tombstones: HashMap<[u8; 16], u64>,
}

impl PinStore {
    pub fn load(data_dir: &Path, enc_key: [u8; 32]) -> Result<Self, PinError> {
        fs::create_dir_all(data_dir).map_err(|e| PinError::Io(e.to_string()))?;
        let path = data_dir.join(PINS_FILE_NAME);

        if !path.exists() {
            return Ok(Self {
                path,
                enc_key,
                items: Vec::new(),
                tombstones: HashMap::new(),
            });
        }

        let raw = fs::read(&path).map_err(|e| PinError::Io(e.to_string()))?;
        if raw.len() < PINS_MAGIC.len() + 12 + 16 {
            return Err(PinError::CorruptStore("file too short"));
        }
        if &raw[..PINS_MAGIC.len()] != PINS_MAGIC {
            return Err(PinError::CorruptStore("invalid magic header"));
        }

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&raw[PINS_MAGIC.len()..PINS_MAGIC.len() + 12]);
        let ciphertext = &raw[PINS_MAGIC.len() + 12..];

        let plaintext = decrypt_aead(&enc_key, &nonce, PINS_MAGIC, ciphertext)?;
        let mut persisted: PersistedPins =
            bincode::deserialize(&plaintext).map_err(|_| PinError::CorruptStore("decode failed"))?;

        sort_and_truncate(&mut persisted.items);

        Ok(Self {
            path,
            enc_key,
            items: persisted.items,
            tombstones: persisted.tombstones,
        })
    }

    pub fn save(&self) -> Result<(), PinError> {
        let persisted = PersistedPins {
            items: self.items.clone(),
            tombstones: self.tombstones.clone(),
        };
        let plaintext =
            bincode::serialize(&persisted).map_err(|_| PinError::CorruptStore("encode failed"))?;
        let nonce = random_nonce();
        let ciphertext = encrypt_aead(&self.enc_key, &nonce, PINS_MAGIC, &plaintext)?;

        let mut out = Vec::with_capacity(PINS_MAGIC.len() + nonce.len() + ciphertext.len());
        out.extend_from_slice(PINS_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);

        atomic_write(&self.path, &out)?;
        Ok(())
    }

    pub fn upsert(&mut self, item: PinItem) -> Result<bool, PinError> {
        if let Some(&deleted_at) = self.tombstones.get(&item.id) {
            if deleted_at >= item.updated_at_ms {
                return Ok(false);
            }
            self.tombstones.remove(&item.id);
        }

        if let Some(existing) = self.items.iter_mut().find(|p| p.id == item.id) {
            if existing.updated_at_ms >= item.updated_at_ms {
                return Ok(false);
            }
            *existing = item;
        } else {
            self.items.push(item);
        }

        sort_and_truncate(&mut self.items);
        self.save()?;
        Ok(true)
    }

    pub fn delete(&mut self, id: &[u8; 16], deleted_at_ms: u64) -> Result<bool, PinError> {
        let prev_tombstone = self.tombstones.get(id).copied().unwrap_or(0);
        if deleted_at_ms > prev_tombstone {
            self.tombstones.insert(*id, deleted_at_ms);
            prune_tombstones(&mut self.tombstones);
        }

        let before = self.items.len();
        self.items.retain(|p| &p.id != id);
        let removed = self.items.len() != before;

        if removed || deleted_at_ms > prev_tombstone {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> Vec<PinItem> {
        self.items.clone()
    }

    pub fn rekey(&mut self, new_enc_key: [u8; 32]) -> Result<(), PinError> {
        self.enc_key = new_enc_key;
        self.items.clear();
        self.tombstones.clear();
        self.save()
    }
}

fn sort_and_truncate(items: &mut Vec<PinItem>) {
    items.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    items.truncate(MAX_PINS);
}

fn prune_tombstones(tombstones: &mut HashMap<[u8; 16], u64>) {
    if tombstones.len() <= MAX_TOMBSTONES {
        return;
    }
    let mut entries: Vec<([u8; 16], u64)> = tombstones.iter().map(|(&k, &v)| (k, v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(MAX_TOMBSTONES);
    *tombstones = entries.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::derive_encryption_key;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clipsync_pins_test_{}_{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn make_pin(id_byte: u8, ts: u64, text: &[u8]) -> PinItem {
        PinItem {
            id: [id_byte; 16],
            kind: ClipKind::Text,
            mime: "text/plain".to_string(),
            data: text.to_vec(),
            created_at_ms: ts,
            updated_at_ms: ts,
        }
    }

    #[test]
    fn encrypted_persistence_and_wrong_key_rejection() {
        let dir = temp_dir("persist");
        let key = derive_encryption_key(&[0x33u8; 32]);
        let secret = b"super-secret-pin-content";

        {
            let mut store = PinStore::load(&dir, key).expect("load empty");
            assert!(store.upsert(make_pin(1, 1000, secret)).expect("upsert"));
        }

        // Plaintext must never appear on disk.
        let on_disk = fs::read(dir.join(PINS_FILE_NAME)).expect("read pins.bin");
        assert!(!on_disk.windows(secret.len()).any(|w| w == secret));

        // Reloading with correct key restores pin.
        let reloaded = PinStore::load(&dir, key).expect("reload");
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].data, secret);

        // Reloading with wrong key fails AEAD authentication.
        let wrong_key = derive_encryption_key(&[0x44u8; 32]);
        assert!(matches!(PinStore::load(&dir, wrong_key), Err(PinError::Crypto(_))));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn enforces_max_25_pins_and_tombstone_blocks_stale_upsert() {
        let dir = temp_dir("cap_tombstone");
        let key = derive_encryption_key(&[0x66u8; 32]);
        let mut store = PinStore::load(&dir, key).expect("load");

        for i in 1..=30u8 {
            store
                .upsert(make_pin(i, 1000 + i as u64, format!("pin-{i}").as_bytes()))
                .expect("upsert");
        }
        let pins = store.list();
        assert_eq!(pins.len(), MAX_PINS);
        assert_eq!(pins[0].id, [30u8; 16]);
        assert_eq!(pins[MAX_PINS - 1].id, [6u8; 16]);

        // Delete pin 30 at ts=2000, then verify stale upsert at ts=1500 is ignored.
        assert!(store.delete(&[30u8; 16], 2000).expect("delete"));
        assert!(!store.upsert(make_pin(30, 1500, b"stale")).expect("stale upsert"));
        assert!(!store.list().iter().any(|p| p.id == [30u8; 16]));

        // Newer upsert after delete succeeds.
        assert!(store.upsert(make_pin(30, 2500, b"newer")).expect("newer upsert"));
        assert_eq!(store.list()[0].data, b"newer");

        let _ = fs::remove_dir_all(&dir);
    }
}
