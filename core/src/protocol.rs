use std::fmt;

use serde::{Deserialize, Serialize};

use crate::crypto::{decrypt_aead, encrypt_aead, random_nonce, CryptoError};

pub const PROTOCOL_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 26;
pub const NONCE_LEN: usize = 12;
pub const AEAD_TAG_LEN: usize = 16;
pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 9 * 1024 * 1024;

pub const TAG_EPHEMERAL_TEXT: u8 = 1;
pub const TAG_EPHEMERAL_IMAGE: u8 = 2;
pub const TAG_PIN_UPSERT: u8 = 3;
pub const TAG_PIN_DELETE: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ClipKind {
    Text = 1,
    Image = 2,
}

impl ClipKind {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(Self::Text),
            2 => Some(Self::Image),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    EphemeralText {
        id: [u8; 16],
        timestamp_ms: u64,
        text: String,
    },
    EphemeralImage {
        id: [u8; 16],
        timestamp_ms: u64,
        mime: String,
        data: Vec<u8>,
    },
    PinUpsert {
        id: [u8; 16],
        kind: ClipKind,
        mime: String,
        data: Vec<u8>,
        created_at_ms: u64,
        updated_at_ms: u64,
    },
    PinDelete {
        id: [u8; 16],
        deleted_at_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    FrameTooShort,
    FrameTooLarge,
    UnsupportedVersion(u8),
    UnknownTag(u8),
    PayloadTooLarge,
    Crypto(CryptoError),
    MalformedPayload(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooShort => write!(f, "wire frame too short"),
            Self::FrameTooLarge => write!(f, "wire frame exceeds max size"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            Self::UnknownTag(t) => write!(f, "unknown protocol tag {t}"),
            Self::PayloadTooLarge => write!(f, "payload exceeds 8 MiB limit"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::MalformedPayload(reason) => write!(f, "malformed payload: {reason}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<CryptoError> for ProtocolError {
    fn from(err: CryptoError) -> Self {
        Self::Crypto(err)
    }
}

#[derive(Serialize, Deserialize)]
struct EphemeralTextPayload {
    text: String,
}

#[derive(Serialize, Deserialize)]
struct EphemeralImagePayload {
    mime: String,
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PinUpsertPayload {
    id: [u8; 16],
    kind: ClipKind,
    mime: String,
    data: Vec<u8>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct PinDeletePayload {
    id: [u8; 16],
    deleted_at_ms: u64,
}

pub fn encode_frame(msg: &WireMessage, enc_key: &[u8; 32]) -> Result<Vec<u8>, ProtocolError> {
    let (tag, id, ts, plaintext) = match msg {
        WireMessage::EphemeralText {
            id,
            timestamp_ms,
            text,
        } => {
            let payload = bincode::serialize(&EphemeralTextPayload { text: text.clone() })
                .map_err(|_| ProtocolError::MalformedPayload("failed to encode text payload"))?;
            (TAG_EPHEMERAL_TEXT, *id, *timestamp_ms, payload)
        }
        WireMessage::EphemeralImage {
            id,
            timestamp_ms,
            mime,
            data,
        } => {
            if data.len() > MAX_PAYLOAD_BYTES {
                return Err(ProtocolError::PayloadTooLarge);
            }
            let payload = bincode::serialize(&EphemeralImagePayload {
                mime: mime.clone(),
                data: data.clone(),
            })
            .map_err(|_| ProtocolError::MalformedPayload("failed to encode image payload"))?;
            (TAG_EPHEMERAL_IMAGE, *id, *timestamp_ms, payload)
        }
        WireMessage::PinUpsert {
            id,
            kind,
            mime,
            data,
            created_at_ms,
            updated_at_ms,
        } => {
            if data.len() > MAX_PAYLOAD_BYTES {
                return Err(ProtocolError::PayloadTooLarge);
            }
            let payload = bincode::serialize(&PinUpsertPayload {
                id: *id,
                kind: *kind,
                mime: mime.clone(),
                data: data.clone(),
                created_at_ms: *created_at_ms,
                updated_at_ms: *updated_at_ms,
            })
            .map_err(|_| ProtocolError::MalformedPayload("failed to encode pin upsert"))?;
            (TAG_PIN_UPSERT, *id, *updated_at_ms, payload)
        }
        WireMessage::PinDelete { id, deleted_at_ms } => {
            let payload = bincode::serialize(&PinDeletePayload {
                id: *id,
                deleted_at_ms: *deleted_at_ms,
            })
            .map_err(|_| ProtocolError::MalformedPayload("failed to encode pin delete"))?;
            (TAG_PIN_DELETE, *id, *deleted_at_ms, payload)
        }
    };

    if plaintext.len() > MAX_PAYLOAD_BYTES + 4096 {
        return Err(ProtocolError::PayloadTooLarge);
    }

    let mut header = [0u8; HEADER_LEN];
    header[0] = PROTOCOL_VERSION;
    header[1] = tag;
    header[2..18].copy_from_slice(&id);
    header[18..26].copy_from_slice(&ts.to_be_bytes());

    let nonce = random_nonce();
    let ciphertext = encrypt_aead(enc_key, &nonce, &header, &plaintext)?;

    let total_len = HEADER_LEN + NONCE_LEN + ciphertext.len();
    if total_len > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }

    let mut frame = Vec::with_capacity(total_len);
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8], enc_key: &[u8; 32]) -> Result<WireMessage, ProtocolError> {
    if frame.len() < HEADER_LEN + NONCE_LEN + AEAD_TAG_LEN {
        return Err(ProtocolError::FrameTooShort);
    }
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }

    let header = &frame[..HEADER_LEN];
    if header[0] != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(header[0]));
    }
    let tag = header[1];

    let mut id = [0u8; 16];
    id.copy_from_slice(&header[2..18]);

    let mut ts_bytes = [0u8; 8];
    ts_bytes.copy_from_slice(&header[18..26]);
    let ts = u64::from_be_bytes(ts_bytes);

    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&frame[HEADER_LEN..HEADER_LEN + NONCE_LEN]);
    let ciphertext = &frame[HEADER_LEN + NONCE_LEN..];

    let plaintext = decrypt_aead(enc_key, &nonce, header, ciphertext)?;

    match tag {
        TAG_EPHEMERAL_TEXT => {
            let payload: EphemeralTextPayload = bincode::deserialize(&plaintext)
                .map_err(|_| ProtocolError::MalformedPayload("invalid text payload"))?;
            Ok(WireMessage::EphemeralText {
                id,
                timestamp_ms: ts,
                text: payload.text,
            })
        }
        TAG_EPHEMERAL_IMAGE => {
            let payload: EphemeralImagePayload = bincode::deserialize(&plaintext)
                .map_err(|_| ProtocolError::MalformedPayload("invalid image payload"))?;
            if payload.data.len() > MAX_PAYLOAD_BYTES {
                return Err(ProtocolError::PayloadTooLarge);
            }
            Ok(WireMessage::EphemeralImage {
                id,
                timestamp_ms: ts,
                mime: payload.mime,
                data: payload.data,
            })
        }
        TAG_PIN_UPSERT => {
            let payload: PinUpsertPayload = bincode::deserialize(&plaintext)
                .map_err(|_| ProtocolError::MalformedPayload("invalid pin upsert payload"))?;
            if payload.id != id || payload.updated_at_ms != ts {
                return Err(ProtocolError::MalformedPayload("pin header mismatch"));
            }
            if payload.data.len() > MAX_PAYLOAD_BYTES {
                return Err(ProtocolError::PayloadTooLarge);
            }
            Ok(WireMessage::PinUpsert {
                id: payload.id,
                kind: payload.kind,
                mime: payload.mime,
                data: payload.data,
                created_at_ms: payload.created_at_ms,
                updated_at_ms: payload.updated_at_ms,
            })
        }
        TAG_PIN_DELETE => {
            let payload: PinDeletePayload = bincode::deserialize(&plaintext)
                .map_err(|_| ProtocolError::MalformedPayload("invalid pin delete payload"))?;
            if payload.id != id || payload.deleted_at_ms != ts {
                return Err(ProtocolError::MalformedPayload("delete header mismatch"));
            }
            Ok(WireMessage::PinDelete {
                id: payload.id,
                deleted_at_ms: payload.deleted_at_ms,
            })
        }
        other => Err(ProtocolError::UnknownTag(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{derive_encryption_key, random_id};

    #[test]
    fn encode_decode_all_tags() {
        let key = derive_encryption_key(&[0x55u8; 32]);
        let id = random_id();

        let messages = vec![
            WireMessage::EphemeralText {
                id,
                timestamp_ms: 1_700_000_000_001,
                text: "Hello ClipSync 🚀".to_string(),
            },
            WireMessage::EphemeralImage {
                id,
                timestamp_ms: 1_700_000_000_002,
                mime: "image/png".to_string(),
                data: vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4],
            },
            WireMessage::PinUpsert {
                id,
                kind: ClipKind::Text,
                mime: "text/plain".to_string(),
                data: b"pinned secret note".to_vec(),
                created_at_ms: 1_700_000_000_000,
                updated_at_ms: 1_700_000_000_003,
            },
            WireMessage::PinDelete {
                id,
                deleted_at_ms: 1_700_000_000_004,
            },
        ];

        for msg in messages {
            let frame = encode_frame(&msg, &key).expect("encode");
            let decoded = decode_frame(&frame, &key).expect("decode");
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn frame_rejects_header_tamper() {
        let key = derive_encryption_key(&[0x77u8; 32]);
        let msg = WireMessage::PinDelete {
            id: [0x12u8; 16],
            deleted_at_ms: 123456,
        };
        let mut frame = encode_frame(&msg, &key).expect("encode");
        frame[25] ^= 0x01; // Flip bit in timestamp header (AAD)
        assert!(matches!(decode_frame(&frame, &key), Err(ProtocolError::Crypto(_))));
    }
}
