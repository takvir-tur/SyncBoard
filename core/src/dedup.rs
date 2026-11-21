use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::protocol::ClipKind;

const DEFAULT_RING_CAPACITY: usize = 64;
const DEFAULT_ECHO_TTL: Duration = Duration::from_secs(10);

pub fn hash_clip(kind: ClipKind, data: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[kind as u8]);
    hasher.update(data);
    *hasher.finalize().as_bytes()
}

pub struct EchoGuard {
    ring: VecDeque<([u8; 32], Instant)>,
    capacity: usize,
    ttl: Duration,
    last_seen: Option<[u8; 32]>,
}

impl Default for EchoGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoGuard {
    pub fn new() -> Self {
        Self::with_capacity_and_ttl(DEFAULT_RING_CAPACITY, DEFAULT_ECHO_TTL)
    }

    pub fn with_capacity_and_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            ring: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            ttl,
            last_seen: None,
        }
    }

    /// Record a hash coming from a remote device (or locally injected clip) so
    /// the local clipboard observer does not echo it back to the cluster.
    pub fn record_remote(&mut self, hash: [u8; 32]) {
        self.record_remote_at(hash, Instant::now());
    }

    fn record_remote_at(&mut self, hash: [u8; 32], now: Instant) {
        self.prune(now);
        if self.ring.len() >= self.capacity {
            self.ring.pop_front();
        }
        self.ring.push_back((hash, now));
        self.last_seen = Some(hash);
    }

    /// Check whether this clip hash matches a recent remote injection or the
    /// immediately preceding clipboard state.
    pub fn should_suppress(&mut self, hash: [u8; 32]) -> bool {
        self.should_suppress_at(hash, Instant::now())
    }

    fn should_suppress_at(&mut self, hash: [u8; 32], now: Instant) -> bool {
        self.prune(now);
        if self.last_seen == Some(hash) {
            return true;
        }
        self.ring.iter().any(|(h, _)| *h == hash)
    }

    /// Returns `true` if `hash` is a genuinely new local clipboard item and records it
    /// as `last_seen`. Returns `false` if it was suppressed as an echo or duplicate.
    pub fn check_and_record_local(&mut self, hash: [u8; 32]) -> bool {
        self.check_and_record_local_at(hash, Instant::now())
    }

    fn check_and_record_local_at(&mut self, hash: [u8; 32], now: Instant) -> bool {
        if self.should_suppress_at(hash, now) {
            return false;
        }
        self.last_seen = Some(hash);
        true
    }

    pub fn record_local(&mut self, hash: [u8; 32]) {
        self.last_seen = Some(hash);
    }

    pub fn clear(&mut self) {
        self.ring.clear();
        self.last_seen = None;
    }

    fn prune(&mut self, now: Instant) {
        while let Some((_, ts)) = self.ring.front() {
            if now.saturating_duration_since(*ts) > self.ttl {
                self.ring.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_remote_echo_and_duplicate_local() {
        let mut guard = EchoGuard::with_capacity_and_ttl(4, Duration::from_secs(10));
        let t0 = Instant::now();
        let h_remote = hash_clip(ClipKind::Text, b"from phone");
        let h_local = hash_clip(ClipKind::Text, b"typed locally");

        guard.record_remote_at(h_remote, t0);
        assert!(guard.should_suppress_at(h_remote, t0 + Duration::from_secs(1)));
        assert!(!guard.check_and_record_local_at(h_remote, t0 + Duration::from_secs(1)));

        assert!(guard.check_and_record_local_at(h_local, t0 + Duration::from_secs(2)));
        assert!(!guard.check_and_record_local_at(h_local, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn ring_buffer_eviction_and_ttl_expiry() {
        let mut guard = EchoGuard::with_capacity_and_ttl(3, Duration::from_millis(50));
        let t0 = Instant::now();
        let h1 = hash_clip(ClipKind::Text, b"1");
        let h2 = hash_clip(ClipKind::Text, b"2");
        let h3 = hash_clip(ClipKind::Text, b"3");
        let h4 = hash_clip(ClipKind::Text, b"4");

        guard.record_remote_at(h1, t0);
        guard.record_remote_at(h2, t0);
        guard.record_remote_at(h3, t0);
        guard.record_remote_at(h4, t0);

        // Capacity is 3 so h1 was evicted.
        assert!(!guard.should_suppress_at(h1, t0));
        assert!(guard.should_suppress_at(h2, t0));
        assert!(guard.should_suppress_at(h4, t0));

        // After TTL expires, ring entries expire; h2 is no longer suppressed once last_seen != h2.
        assert!(!guard.should_suppress_at(h2, t0 + Duration::from_millis(100)));
    }
}
