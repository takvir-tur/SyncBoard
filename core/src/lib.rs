pub mod crypto;
pub mod dedup;
pub mod ffi;
pub mod media;
pub mod pins;
pub mod protocol;
pub mod sync;

#[cfg(target_os = "android")]
pub mod jni_bridge;

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crypto::{
    derive_encryption_key, derive_room_id, load_or_create_config, now_millis, parse_pairing_uri,
    random_id, save_config, ClusterConfig, CryptoError,
};
use dedup::{hash_clip, EchoGuard};
use media::{normalize_and_compress_image, MediaError};
use pins::{PinError, PinItem, PinStore};
use protocol::{encode_frame, ClipKind, ProtocolError, WireMessage, MAX_PAYLOAD_BYTES};
use sync::{spawn_sync_worker, RemoteClip, SharedState, SyncCommand, SyncHandle};

pub const DEFAULT_RELAY_URL: &str = "wss://clipsync-relay.takvir-tur.workers.dev";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    InvalidInput(&'static str),
    Crypto(CryptoError),
    Protocol(ProtocolError),
    Media(MediaError),
    Pins(PinError),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::Protocol(e) => write!(f, "{e}"),
            Self::Media(e) => write!(f, "{e}"),
            Self::Pins(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<CryptoError> for CoreError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl From<ProtocolError> for CoreError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

impl From<MediaError> for CoreError {
    fn from(e: MediaError) -> Self {
        Self::Media(e)
    }
}

impl From<PinError> for CoreError {
    fn from(e: PinError) -> Self {
        Self::Pins(e)
    }
}

pub struct Engine {
    data_dir: PathBuf,
    shared: Arc<SharedState>,
    worker: Mutex<Option<SyncHandle>>,
}

impl Engine {
    pub fn new(
        data_dir: impl AsRef<Path>,
        default_relay_url: Option<&str>,
    ) -> Result<Self, CoreError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let relay = match default_relay_url {
            Some(url) if !url.trim().is_empty() => url.trim(),
            _ => DEFAULT_RELAY_URL,
        };

        let config = load_or_create_config(&data_dir, relay)?;
        let enc_key = config.encryption_key();
        let room_hex = config.room_id_hex();
        let relay_url = config.relay_url.clone();
        let pins = PinStore::load(&data_dir, enc_key)?;

        let shared = Arc::new(SharedState {
            config: Mutex::new(config),
            enc_key: Mutex::new(enc_key),
            pins: Mutex::new(pins),
            dedup: Mutex::new(EchoGuard::new()),
            inbound: Mutex::new(VecDeque::new()),
            event_signal: (Mutex::new(0), Condvar::new()),
            event_callback: Mutex::new(None),
        });

        let handle = spawn_sync_worker(Arc::clone(&shared), relay_url, room_hex, enc_key);

        Ok(Self {
            data_dir,
            shared,
            worker: Mutex::new(Some(handle)),
        })
    }

    pub fn push_local(&self, kind: ClipKind, data: &[u8]) -> Result<bool, CoreError> {
        if data.is_empty() {
            return Ok(false);
        }

        let enc_key = *self.shared.enc_key.lock().unwrap();
        let msg = match kind {
            ClipKind::Text => {
                if data.len() > MAX_PAYLOAD_BYTES {
                    return Err(CoreError::InvalidInput("text exceeds 8 MiB limit"));
                }
                let text = std::str::from_utf8(data)
                    .map_err(|_| CoreError::InvalidInput("text must be valid UTF-8"))?;
                let hash = hash_clip(ClipKind::Text, data);
                if !self.shared.dedup.lock().unwrap().check_and_record_local(hash) {
                    return Ok(false);
                }
                WireMessage::EphemeralText {
                    id: random_id(),
                    timestamp_ms: now_millis(),
                    text: text.to_string(),
                }
            }
            ClipKind::Image => {
                let raw_hash = hash_clip(ClipKind::Image, data);
                {
                    let mut dedup = self.shared.dedup.lock().unwrap();
                    if dedup.should_suppress(raw_hash) {
                        return Ok(false);
                    }
                }
                let compressed = normalize_and_compress_image(data)?;
                let norm_hash = hash_clip(ClipKind::Image, &compressed.data);
                {
                    let mut dedup = self.shared.dedup.lock().unwrap();
                    if !dedup.check_and_record_local(norm_hash) {
                        return Ok(false);
                    }
                    dedup.record_local(raw_hash);
                }
                WireMessage::EphemeralImage {
                    id: random_id(),
                    timestamp_ms: now_millis(),
                    mime: compressed.mime,
                    data: compressed.data,
                }
            }
        };

        let frame = encode_frame(&msg, &enc_key)?;
        if let Some(ref handle) = *self.worker.lock().unwrap() {
            handle.send(SyncCommand::SendFrame(frame));
        }
        Ok(true)
    }

    pub fn pop_remote(&self) -> Option<RemoteClip> {
        self.shared.inbound.lock().unwrap().pop_front()
    }

    pub fn wait_event(&self, timeout_ms: u64) -> bool {
        self.shared
            .wait_for_event(Duration::from_millis(timeout_ms))
    }

    pub fn set_event_callback(&self, cb: Option<Box<dyn Fn() + Send + Sync>>) {
        *self.shared.event_callback.lock().unwrap() = cb;
    }

    pub fn record_echo(&self, kind: ClipKind, data: &[u8]) {
        let hash = hash_clip(kind, data);
        self.shared.dedup.lock().unwrap().record_remote(hash);
    }

    pub fn pin_item(&self, kind: ClipKind, data: &[u8]) -> Result<PinItem, CoreError> {
        if data.is_empty() {
            return Err(CoreError::InvalidInput("cannot pin empty item"));
        }

        let (mime, normalized_data) = match kind {
            ClipKind::Text => {
                if data.len() > MAX_PAYLOAD_BYTES {
                    return Err(CoreError::InvalidInput("text exceeds 8 MiB limit"));
                }
                std::str::from_utf8(data)
                    .map_err(|_| CoreError::InvalidInput("text must be valid UTF-8"))?;
                ("text/plain;charset=utf-8".to_string(), data.to_vec())
            }
            ClipKind::Image => {
                let compressed = normalize_and_compress_image(data)?;
                (compressed.mime, compressed.data)
            }
        };

        let now = now_millis();
        let item = PinItem {
            id: random_id(),
            kind,
            mime,
            data: normalized_data,
            created_at_ms: now,
            updated_at_ms: now,
        };

        self.shared.pins.lock().unwrap().upsert(item.clone())?;

        let enc_key = *self.shared.enc_key.lock().unwrap();
        let frame = encode_frame(
            &WireMessage::PinUpsert {
                id: item.id,
                kind: item.kind,
                mime: item.mime.clone(),
                data: item.data.clone(),
                created_at_ms: item.created_at_ms,
                updated_at_ms: item.updated_at_ms,
            },
            &enc_key,
        )?;

        if let Some(ref handle) = *self.worker.lock().unwrap() {
            handle.send(SyncCommand::SendFrame(frame));
        }
        self.shared.notify_event();
        Ok(item)
    }

    pub fn unpin_item(&self, id: &[u8; 16]) -> Result<bool, CoreError> {
        let now = now_millis();
        let removed = self.shared.pins.lock().unwrap().delete(id, now)?;

        let enc_key = *self.shared.enc_key.lock().unwrap();
        let frame = encode_frame(
            &WireMessage::PinDelete {
                id: *id,
                deleted_at_ms: now,
            },
            &enc_key,
        )?;
        if let Some(ref handle) = *self.worker.lock().unwrap() {
            handle.send(SyncCommand::SendFrame(frame));
        }
        self.shared.notify_event();
        Ok(removed)
    }

    pub fn get_pins(&self) -> Vec<PinItem> {
        self.shared.pins.lock().unwrap().list()
    }

    pub fn get_pairing_uri(&self) -> String {
        self.shared.config.lock().unwrap().pairing_uri()
    }

    pub fn join_pairing_uri(&self, uri: &str) -> Result<String, CoreError> {
        let (cluster_key, relay_url) = parse_pairing_uri(uri)?;
        let new_config = ClusterConfig {
            cluster_key,
            relay_url: relay_url.clone(),
        };
        let new_enc_key = derive_encryption_key(&cluster_key);
        let new_room_hex = hex::encode(derive_room_id(&cluster_key));

        save_config(&self.data_dir, &new_config)?;
        *self.shared.config.lock().unwrap() = new_config.clone();
        *self.shared.enc_key.lock().unwrap() = new_enc_key;
        self.shared.pins.lock().unwrap().rekey(new_enc_key)?;
        self.shared.dedup.lock().unwrap().clear();
        self.shared.inbound.lock().unwrap().clear();

        if let Some(ref handle) = *self.worker.lock().unwrap() {
            handle.send(SyncCommand::Reconfigure {
                relay_url,
                room_id_hex: new_room_hex,
                enc_key: new_enc_key,
            });
        }
        self.shared.notify_event();
        Ok(new_config.pairing_uri())
    }

    pub fn shutdown(&self) {
        if let Some(mut handle) = self.worker.lock().unwrap().take() {
            handle.shutdown();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn default_relay_url_is_baked_in() {
        assert_eq!(
            DEFAULT_RELAY_URL,
            "wss://clipsync-relay.takvir-tur.workers.dev"
        );
        let dir = std::env::temp_dir().join(format!(
            "clipsync_default_relay_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let engine = Engine::new(&dir, None).expect("init engine with baked-in relay");
        let uri = engine.get_pairing_uri();
        assert!(uri.contains("wss%3A%2F%2Fclipsync-relay.takvir-tur.workers.dev"));
        engine.shutdown();
        let _ = fs::remove_dir_all(&dir);
    }
}
