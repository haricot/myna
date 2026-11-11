//! Explicit, per-user egui dictation anchor.
//!
//! A client must actively arm one document before any transcript is sent.
//! The channel is separate from the status D-Bus API, so text is never
//! broadcast to observers. Neither AT-SPI nor keyboard simulation is used.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt};
use myna_platform::text_input::{Activation, FocusEvent, InjectError, Injector, Target, TextInputCapabilities};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{unix::OwnedReadHalf, unix::OwnedWriteHalf, UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch, Mutex};

const PROTOCOL: u64 = 1;
const MAX_FRAME: usize = 8192;
const MAX_COMMIT: usize = 4096;
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

pub fn socket_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("MYNA_EGUI_ANCHOR_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is required"))?;
    Ok(PathBuf::from(runtime).join("myna-egui-anchor.sock"))
}

fn segments(text: &str) -> impl Iterator<Item = &str> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset == text.len() { return None; }
        let mut end = (offset + MAX_COMMIT).min(text.len());
        while !text.is_char_boundary(end) { end -= 1; }
        let result = &text[offset..end];
        offset = end;
        Some(result)
    })
}

enum Outbound {
    Commit { token: u64, text: String, reply: oneshot::Sender<bool> },
    Preview { token: u64, text: String },
}

struct Armed {
    token: u64,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Sender<bool>,
    target: Option<watch::Sender<bool>>,
}

#[derive(Default)]
struct State { next_token: u64, armed: Option<Armed> }

#[derive(Clone, Default)]
pub struct EguiHub(Arc<Mutex<State>>);

impl EguiHub {
    async fn register(&self, send: mpsc::Sender<Outbound>) -> (u64, watch::Receiver<bool>) {
        let mut state = self.0.lock().await;
        if let Some(old) = state.armed.take() {
            let _ = old.revoke.send(true);
            if let Some(target) = old.target { let _ = target.send(true); }
        }
        state.next_token = state.next_token.wrapping_add(1);
        let token = state.next_token;
        let (revoke, rx) = watch::channel(false);
        state.armed = Some(Armed { token, send, revoke, target: None });
        (token, rx)
    }

    async fn invalidate(&self, token: u64) {
        let mut state = self.0.lock().await;
        if state.armed.as_ref().is_some_and(|armed| armed.token == token) {
            if let Some(old) = state.armed.take() {
                let _ = old.revoke.send(true);
                if let Some(target) = old.target { let _ = target.send(true); }
            }
        }
    }

    pub async fn acquire(&self) -> Option<EguiTarget> {
        let mut state = self.0.lock().await;
        let armed = state.armed.as_mut()?;
        if *armed.revoke.borrow() { return None; }
        if let Some(old) = armed.target.take() { let _ = old.send(true); }
        let (revoke, rx) = watch::channel(false);
        armed.target = Some(revoke);
        Some(EguiTarget { token: armed.token, hub: self.clone(), send: armed.send.clone(), revoke: rx })
    }

    /// Never remove a preexisting socket. Its owner might be a live daemon.
    pub async fn serve(&self, path: PathBuf) -> io::Result<()> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "egui socket needs a private runtime directory"));
        }
        let owner_uid = metadata.uid();
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let _guard = SocketGuard(path);
        loop {
            let (conn, _) = listener.accept().await?;
            // Same-user clients only. This is not a privilege boundary
            // against arbitrary code already running under that same UID.
            if conn.peer_cred()?.uid() != owner_uid { continue; }
            let hub = self.clone();
            tokio::spawn(async move {
                // Never log a transcript, request frame or token.
                let _ = handle_connection(hub, conn).await;
            });
        }
    }
}

struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

async fn read_frame(reader: &mut OwnedReadHalf) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let byte = match reader.read_u8().await {
            Ok(byte) => byte,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(err) => return Err(err),
        };
        if byte == b'\n' { break; }
        if bytes.len() == MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "egui frame too large"));
        }
        bytes.push(byte);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other)
}

async fn write_frame(writer: &mut OwnedWriteHalf, message: Value) -> io::Result<()> {
    let frame = message.to_string();
    if frame.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "egui protocol frame too large"));
    }
    writer.write_all(frame.as_bytes()).await?;
    writer.write_all(b"\n").await
}

fn session_id(message: &Value) -> Option<&str> {
    message.get("session_id").and_then(Value::as_str)
        .filter(|id| id.len() == 36 && id.is_ascii()
            && id.bytes().enumerate().all(|(i, b)| if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else { b.is_ascii_hexdigit() }))
}

async fn handle_connection(hub: EguiHub, conn: UnixStream) -> io::Result<()> {
    let (mut reader, mut writer) = conn.into_split();
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<Value>(8);
    let reader_task = tokio::spawn(async move {
        while let Ok(Some(message)) = read_frame(&mut reader).await {
            if incoming_tx.send(message).await.is_err() { break; }
        }
    });
    let first = tokio::time::timeout(Duration::from_secs(2), incoming_rx.recv()).await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "egui handshake timeout"))?
        .ok_or(io::ErrorKind::UnexpectedEof)?;
    if first.get("type").and_then(Value::as_str) != Some("arm")
        || first.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL)
        || first.as_object().is_none_or(|obj| obj.len() != 3) {
        reader_task.abort();
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad egui handshake"));
    }
    let id = session_id(&first).ok_or(io::ErrorKind::InvalidData)?.to_owned();
    let (send, mut requests) = mpsc::channel::<Outbound>(8);
    let (token, mut revoked) = hub.register(send).await;
    let result = async {
        write_frame(&mut writer, json!({"type":"armed", "protocol":PROTOCOL, "session_id":id})).await?;
        let mut sequence = 0_u64;
        loop {
            tokio::select! {
                changed = revoked.changed() => {
                    if changed.is_err() || *revoked.borrow() { break; }
                }
                message = incoming_rx.recv() => {
                    let Some(message) = message else { break; };
                    // Only explicit disarm is allowed when no commit is pending.
                    if message.as_object().is_some_and(|o| o.len() == 3)
                        && message.get("type").and_then(Value::as_str) == Some("disarm")
                        && message.get("protocol").and_then(Value::as_u64) == Some(PROTOCOL)
                        && session_id(&message) == Some(id.as_str()) { break; }
                    break;
                }
                request = requests.recv() => {
                    let Some(request) = request else { break; };
                    if let Outbound::Preview { token: request_token, text } = request {
                        if request_token == token {
                            write_frame(&mut writer, json!({"type":"preview", "protocol":PROTOCOL,
                                "session_id":id, "sequence":sequence, "text":text})).await?;
                        }
                        continue;
                    }
                    let Outbound::Commit { token: request_token, text, reply } = request else { unreachable!() };
                    if request_token != token { let _ = reply.send(false); continue; }
                    write_frame(&mut writer, json!({"type":"commit", "protocol":PROTOCOL,
                        "session_id":id, "sequence":sequence, "text":text})).await?;
                    let accepted = match tokio::time::timeout(ACK_TIMEOUT, incoming_rx.recv()).await {
                        Ok(Some(message)) => message.as_object().is_some_and(|obj| obj.len() == 5)
                            && message.get("type").and_then(Value::as_str) == Some("commit-result")
                            && message.get("protocol").and_then(Value::as_u64) == Some(PROTOCOL)
                            && session_id(&message) == Some(id.as_str())
                            && message.get("sequence").and_then(Value::as_u64) == Some(sequence)
                            && message.get("ok").and_then(Value::as_bool) == Some(true),
                        _ => false,
                    };
                    let _ = reply.send(accepted);
                    if !accepted { break; }
                    sequence = sequence.wrapping_add(1);
                }
            }
        }
        Ok(())
    }.await;
    hub.invalidate(token).await;
    reader_task.abort();
    result
}

pub struct EguiTarget {
    token: u64,
    hub: EguiHub,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Receiver<bool>,
}

impl std::fmt::Debug for EguiTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("EguiTarget") }
}

#[async_trait]
impl Target for EguiTarget {
    async fn commit(&mut self, text: &str) -> Result<(), InjectError> {
        if *self.revoke.borrow() { return Err(InjectError::FocusLost); }
        for segment in segments(text) {
            let (reply, answer) = oneshot::channel();
            self.send.send(Outbound::Commit { token: self.token, text: segment.to_owned(), reply }).await
                .map_err(|_| InjectError::FocusLost)?;
            match tokio::time::timeout(ACK_TIMEOUT + Duration::from_secs(1), answer).await {
                Ok(Ok(true)) if !*self.revoke.borrow() => {},
                _ => {
                    self.hub.invalidate(self.token).await;
                    return Err(InjectError::FocusLost);
                }
            }
        }
        Ok(())
    }

    async fn set_preedit(&mut self, text: &str) {
        if *self.revoke.borrow() { return; }
        // Preview is volatile and never inserted into the stable document.
        // Bound it to a worst-case 4 KiB of UTF-8, including non-ASCII text.
        let limited: String = text.chars().take(1000).collect();
        let _ = self.send.try_send(Outbound::Preview { token: self.token, text: limited });
    }

    fn focus_events(&self) -> BoxStream<'static, FocusEvent> {
        let mut revoked = self.revoke.clone();
        stream::once(async move {
            loop {
                if *revoked.borrow() || revoked.changed().await.is_err() {
                    return FocusEvent::TargetGone;
                }
            }
        }).boxed()
    }

    async fn release(self: Box<Self>) {
        // Retain explicit authorization until disarm or connection loss.
    }
}

/// A user-armed egui document takes priority; the existing Firefox/IBus
/// injector is unchanged for all other activations.
pub struct EguiOrInjector<I> { hub: EguiHub, fallback: I }

impl<I> EguiOrInjector<I> {
    pub fn new(hub: EguiHub, fallback: I) -> Self { Self { hub, fallback } }
}

#[async_trait]
impl<I: Injector> Injector for EguiOrInjector<I> {
    async fn acquire(&mut self) -> Result<Box<dyn Target>, InjectError> {
        if let Some(target) = self.hub.acquire().await { return Ok(Box::new(target)); }
        self.fallback.acquire().await
    }
    fn capabilities(&self) -> TextInputCapabilities { self.fallback.capabilities() }
    fn activation(&self) -> Activation { self.fallback.activation() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test]
    async fn no_text_target_without_explicit_arm() {
        assert!(EguiHub::default().acquire().await.is_none());
    }

    #[tokio::test]
    async fn armed_client_receives_commits_and_wrong_ack_revokes() {
        let hub = EguiHub::default();
        let (daemon, app) = UnixStream::pair().unwrap();
        let task = tokio::spawn(handle_connection(hub.clone(), daemon));
        let (input, mut output) = app.into_split();
        let mut reader = BufReader::new(input);
        let id = "12345678-1234-1234-1234-123456789abc";
        output.write_all(format!("{{\"type\":\"arm\",\"protocol\":1,\"session_id\":\"{id}\"}}\n").as_bytes()).await.unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(),
            json!({"type":"armed", "protocol":1, "session_id":id}));
        let mut target = hub.acquire().await.expect("armed");
        let commit = tokio::spawn(async move { target.commit("bonjour").await });
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(),
            json!({"type":"commit", "protocol":1, "session_id":id, "sequence":0, "text":"bonjour"}));
        output.write_all(format!("{{\"type\":\"commit-result\",\"protocol\":1,\"session_id\":\"{id}\",\"sequence\":99,\"ok\":true}}\n").as_bytes()).await.unwrap();
        assert!(matches!(commit.await.unwrap(), Err(InjectError::FocusLost)));
        assert!(hub.acquire().await.is_none());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn preview_is_volatile_and_stable_commit_needs_ack() {
        let hub = EguiHub::default();
        let (daemon, app) = UnixStream::pair().unwrap();
        let task = tokio::spawn(handle_connection(hub.clone(), daemon));
        let (input, mut output) = app.into_split();
        let mut reader = BufReader::new(input);
        let id = "12345678-1234-1234-1234-123456789abc";
        output.write_all(format!("{{\"type\":\"arm\",\"protocol\":1,\"session_id\":\"{id}\"}}\n").as_bytes()).await.unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let mut target = hub.acquire().await.unwrap();
        target.set_preedit("Bonjour peut-").await;
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(),
            json!({"type":"preview", "protocol":1, "session_id":id, "sequence":0, "text":"Bonjour peut-"}));
        let commit = tokio::spawn(async move { target.commit("Bonjour !").await });
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["text"], "Bonjour !");
        output.write_all(format!("{{\"type\":\"commit-result\",\"protocol\":1,\"session_id\":\"{id}\",\"sequence\":0,\"ok\":true}}\n").as_bytes()).await.unwrap();
        assert!(commit.await.unwrap().is_ok());
        drop(output);
        drop(reader);
        task.await.unwrap().unwrap();
    }

    #[test]
    fn chunks_are_utf8_safe() {
        let text = "é😀".repeat(2000);
        let chunks: Vec<_> = segments(&text).collect();
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.len() <= MAX_COMMIT));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn session_id_must_be_a_uuid_not_an_arbitrary_string() {
        assert!(session_id(&json!({"session_id":"../../etc/passwd"})).is_none());
        assert!(session_id(&json!({"session_id":"12345678-1234-1234-1234-123456789abc"})).is_some());
    }
}
