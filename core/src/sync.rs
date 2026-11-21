use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::crypto::{normalize_relay_url, ClusterConfig};
use crate::dedup::{hash_clip, EchoGuard};
use crate::pins::{PinItem, PinStore};
use crate::protocol::{decode_frame, encode_frame, ClipKind, WireMessage};

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(45);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_INBOUND_QUEUE: usize = 32;
const MAX_PENDING_OUTBOUND: usize = 32;

#[derive(Debug, Clone)]
pub struct RemoteClip {
    pub kind: ClipKind,
    pub mime: String,
    pub data: Vec<u8>,
    pub timestamp_ms: u64,
}

pub(crate) enum SyncCommand {
    SendFrame(Vec<u8>),
    Reconfigure {
        relay_url: String,
        room_id_hex: String,
        enc_key: [u8; 32],
    },
    Shutdown,
}

pub(crate) struct SharedState {
    pub config: Mutex<ClusterConfig>,
    pub enc_key: Mutex<[u8; 32]>,
    pub pins: Mutex<PinStore>,
    pub dedup: Mutex<EchoGuard>,
    pub inbound: Mutex<VecDeque<RemoteClip>>,
    pub event_signal: (Mutex<u64>, Condvar),
    pub event_callback: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl SharedState {
    pub fn notify_event(&self) {
        {
            let (lock, cvar) = &self.event_signal;
            let mut gen = lock.lock().unwrap();
            *gen = gen.wrapping_add(1);
            cvar.notify_all();
        }
        if let Some(ref cb) = *self.event_callback.lock().unwrap() {
            cb();
        }
    }

    pub fn wait_for_event(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &self.event_signal;
        let mut gen = lock.lock().unwrap();
        let start_gen = *gen;
        if !self.inbound.lock().unwrap().is_empty() {
            return true;
        }
        let (guard, _) = cvar
            .wait_timeout_while(gen, timeout, |g| *g == start_gen)
            .unwrap();
        gen = guard;
        *gen != start_gen || !self.inbound.lock().unwrap().is_empty()
    }
}

pub(crate) struct SyncHandle {
    cmd_tx: mpsc::UnboundedSender<SyncCommand>,
    thread: Option<JoinHandle<()>>,
}

impl SyncHandle {
    pub fn send(&self, cmd: SyncCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(SyncCommand::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn spawn_sync_worker(
    shared: Arc<SharedState>,
    initial_relay: String,
    initial_room: String,
    initial_key: [u8; 32],
) -> SyncHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    let thread = thread::Builder::new()
        .name("clipsync-net".to_string())
        .spawn(move || {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio current_thread runtime");

            rt.block_on(worker_loop(
                shared,
                cmd_rx,
                initial_relay,
                initial_room,
                initial_key,
            ));
        })
        .expect("spawn clipsync-net thread");

    SyncHandle {
        cmd_tx,
        thread: Some(thread),
    }
}

async fn worker_loop(
    shared: Arc<SharedState>,
    mut cmd_rx: mpsc::UnboundedReceiver<SyncCommand>,
    mut relay_url: String,
    mut room_id_hex: String,
    mut enc_key: [u8; 32],
) {
    let mut backoff = Duration::from_secs(1);
    let mut pending_outbound: VecDeque<Vec<u8>> = VecDeque::new();

    loop {
        let ws_url = build_ws_url(&relay_url, &room_id_hex);
        let connect_fut = connect_async(&ws_url);
        tokio::pin!(connect_fut);

        let mut reconnect_immediately = false;
        let mut connected_stream = None;

        // Allow Shutdown / Reconfigure commands to interrupt in-flight connect attempts.
        loop {
            tokio::select! {
                res = &mut connect_fut => {
                    match res {
                        Ok((stream, _)) => {
                            connected_stream = Some(stream);
                        }
                        Err(_) => {}
                    }
                    break;
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::SendFrame(frame)) => {
                            push_pending(&mut pending_outbound, frame);
                        }
                        Some(SyncCommand::Reconfigure { relay_url: r, room_id_hex: room, enc_key: k }) => {
                            relay_url = r;
                            room_id_hex = room;
                            enc_key = k;
                            pending_outbound.clear();
                            backoff = Duration::from_secs(1);
                            reconnect_immediately = true;
                            break;
                        }
                        Some(SyncCommand::Shutdown) | None => return,
                    }
                }
            }
        }

        if reconnect_immediately {
            continue;
        }

        if let Some(ws_stream) = connected_stream {
            backoff = Duration::from_secs(1);
            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // Replay locally persisted pins so offline pins sync on reconnect.
            let pin_frames = build_pin_replay_frames(&shared, &enc_key);
            let mut send_ok = true;
            for frame in pin_frames {
                if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                    send_ok = false;
                    break;
                }
            }

            while send_ok {
                let Some(frame) = pending_outbound.pop_front() else {
                    break;
                };
                if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                    send_ok = false;
                    break;
                }
            }

            if send_ok {
                let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // Consume immediate first tick.
                keepalive.tick().await;

                loop {
                    tokio::select! {
                        cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(SyncCommand::SendFrame(frame)) => {
                                    if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Some(SyncCommand::Reconfigure { relay_url: r, room_id_hex: room, enc_key: k }) => {
                                    relay_url = r;
                                    room_id_hex = room;
                                    enc_key = k;
                                    pending_outbound.clear();
                                    backoff = Duration::from_secs(1);
                                    reconnect_immediately = true;
                                    let _ = ws_tx.close().await;
                                    break;
                                }
                                Some(SyncCommand::Shutdown) | None => {
                                    let _ = ws_tx.close().await;
                                    return;
                                }
                            }
                        }
                        incoming = ws_rx.next() => {
                            match incoming {
                                Some(Ok(Message::Binary(bytes))) => {
                                    handle_incoming_frame(&shared, &bytes, &enc_key);
                                }
                                Some(Ok(Message::Ping(payload))) => {
                                    if ws_tx.send(Message::Pong(payload)).await.is_err() {
                                        break;
                                    }
                                }
                                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                                    break;
                                }
                                Some(Ok(_)) => {}
                            }
                        }
                        _ = keepalive.tick() => {
                            if ws_tx.send(Message::Text("ping".into())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if reconnect_immediately {
            continue;
        }

        // Wait with exponential backoff while still honoring Shutdown / Reconfigure.
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => {
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    break;
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::SendFrame(frame)) => {
                            push_pending(&mut pending_outbound, frame);
                        }
                        Some(SyncCommand::Reconfigure { relay_url: r, room_id_hex: room, enc_key: k }) => {
                            relay_url = r;
                            room_id_hex = room;
                            enc_key = k;
                            pending_outbound.clear();
                            backoff = Duration::from_secs(1);
                            break;
                        }
                        Some(SyncCommand::Shutdown) | None => return,
                    }
                }
            }
        }
    }
}

fn push_pending(queue: &mut VecDeque<Vec<u8>>, frame: Vec<u8>) {
    if queue.len() >= MAX_PENDING_OUTBOUND {
        queue.pop_front();
    }
    queue.push_back(frame);
}

fn build_ws_url(relay_url: &str, room_id_hex: &str) -> String {
    let base = normalize_relay_url(relay_url);
    format!("{base}/room/{room_id_hex}")
}

fn build_pin_replay_frames(shared: &SharedState, enc_key: &[u8; 32]) -> Vec<Vec<u8>> {
    let pins = shared.pins.lock().unwrap().list();
    let mut frames = Vec::with_capacity(pins.len());
    for pin in pins {
        let msg = WireMessage::PinUpsert {
            id: pin.id,
            kind: pin.kind,
            mime: pin.mime,
            data: pin.data,
            created_at_ms: pin.created_at_ms,
            updated_at_ms: pin.updated_at_ms,
        };
        if let Ok(frame) = encode_frame(&msg, enc_key) {
            frames.push(frame);
        }
    }
    frames
}

fn handle_incoming_frame(shared: &SharedState, bytes: &[u8], enc_key: &[u8; 32]) {
    let Ok(msg) = decode_frame(bytes, enc_key) else {
        return;
    };

    match msg {
        WireMessage::EphemeralText {
            timestamp_ms, text, ..
        } => {
            let data = text.into_bytes();
            let hash = hash_clip(ClipKind::Text, &data);
            shared.dedup.lock().unwrap().record_remote(hash);
            {
                let mut inbound = shared.inbound.lock().unwrap();
                if inbound.len() >= MAX_INBOUND_QUEUE {
                    inbound.pop_front();
                }
                inbound.push_back(RemoteClip {
                    kind: ClipKind::Text,
                    mime: "text/plain;charset=utf-8".to_string(),
                    data,
                    timestamp_ms,
                });
            }
            shared.notify_event();
        }
        WireMessage::EphemeralImage {
            timestamp_ms,
            mime,
            data,
            ..
        } => {
            let hash = hash_clip(ClipKind::Image, &data);
            shared.dedup.lock().unwrap().record_remote(hash);
            {
                let mut inbound = shared.inbound.lock().unwrap();
                if inbound.len() >= MAX_INBOUND_QUEUE {
                    inbound.pop_front();
                }
                inbound.push_back(RemoteClip {
                    kind: ClipKind::Image,
                    mime,
                    data,
                    timestamp_ms,
                });
            }
            shared.notify_event();
        }
        WireMessage::PinUpsert {
            id,
            kind,
            mime,
            data,
            created_at_ms,
            updated_at_ms,
        } => {
            let changed = shared
                .pins
                .lock()
                .unwrap()
                .upsert(PinItem {
                    id,
                    kind,
                    mime,
                    data,
                    created_at_ms,
                    updated_at_ms,
                })
                .unwrap_or(false);
            if changed {
                shared.notify_event();
            }
        }
        WireMessage::PinDelete { id, deleted_at_ms } => {
            let changed = shared
                .pins
                .lock()
                .unwrap()
                .delete(&id, deleted_at_ms)
                .unwrap_or(false);
            if changed {
                shared.notify_event();
            }
        }
    }
}
