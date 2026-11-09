//! Opt-in, one-field-at-a-time Firefox injection target, without AT-SPI.
//!
//! The session daemon owns a Unix socket in its user's private runtime dir.
//! Firefox's restricted native host relays only arm, commit and ack messages.
//! No document content is ever requested from Firefox.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt};
use myna_platform::text_input::{Activation, FocusEvent, InjectError, Injector, Target, TextInputCapabilities};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{unix::OwnedReadHalf, UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch, Mutex};

const MAX_FRAME: usize = 8 * 1024;
const ACK_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_COMMIT_BYTES: usize = 4096;

fn segments(text: &str) -> impl Iterator<Item = &str> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if offset >= text.len() { return None; }
        let mut end = (offset + MAX_COMMIT_BYTES).min(text.len());
        while !text.is_char_boundary(end) { end -= 1; }
        let segment = &text[offset..end];
        offset = end;
        Some(segment)
    })
}

pub fn socket_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("MYNA_FIREFOX_ANCHOR_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is required"))?;
    Ok(PathBuf::from(runtime).join("myna-firefox-anchor.sock"))
}

struct Outbound {
    text: String,
    result: oneshot::Sender<bool>,
}

struct Armed {
    token: u64,
    session_id: String,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Sender<bool>,
    target: Option<watch::Sender<bool>>,
}

#[derive(Default)]
struct State {
    next_token: u64,
    armed: Option<Armed>,
}

#[derive(Clone, Default)]
pub struct FirefoxHub(Arc<Mutex<State>>);

impl FirefoxHub {
    async fn register(&self, session_id: String, send: mpsc::Sender<Outbound>) -> (u64, watch::Receiver<bool>) {
        let mut state = self.0.lock().await;
        if let Some(old) = state.armed.take() {
            let _ = old.revoke.send(true);
            if let Some(target) = old.target { let _ = target.send(true); }
        }
        state.next_token += 1;
        let token = state.next_token;
        let (revoke, receiver) = watch::channel(false);
        state.armed = Some(Armed { token, session_id, send, revoke, target: None });
        (token, receiver)
    }

    async fn invalidate(&self, token: u64) {
        let mut state = self.0.lock().await;
        if state.armed.as_ref().is_some_and(|a| a.token == token) {
            if let Some(old) = state.armed.take() {
                let _ = old.revoke.send(true);
                if let Some(target) = old.target { let _ = target.send(true); }
            }
        }
    }

    async fn acquire(&self) -> Option<FirefoxTarget> {
        let mut state = self.0.lock().await;
        let armed = state.armed.as_mut()?;
        if *armed.revoke.borrow() { return None; }
        if let Some(previous) = armed.target.take() { let _ = previous.send(true); }
        let (revoke, receiver) = watch::channel(false);
        armed.target = Some(revoke);
        Some(FirefoxTarget { token: armed.token, hub: self.clone(), send: armed.send.clone(), revoke: receiver })
    }

    /// Bind only when opted in by the daemon. No unlink-on-bind: a conflicting
    /// socket is never deleted (it might belong to another live process).
    pub async fn serve(&self, path: PathBuf) -> io::Result<()> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let metadata = std::fs::metadata(parent)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "Firefox socket requires a private (0700) runtime directory"));
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let _guard = SocketGuard(path);
        loop {
            let (conn, _) = listener.accept().await?;
            let hub = self.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(hub, conn).await {
                    // Never log transcript text or full protocol messages.
                    myna_core::dbg_log!("inject", "Firefox session closed: {err}");
                }
            });
        }
    }
}

struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

async fn read_message(input: &mut OwnedReadHalf) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let byte = match input.read_u8().await {
            Ok(byte) => byte,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(err) => return Err(err),
        };
        if byte == b'\n' { break; }
        if bytes.len() >= MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Firefox message too large"));
        }
        bytes.push(byte);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other)
}

async fn handle_connection(hub: FirefoxHub, conn: UnixStream) -> io::Result<()> {
    let (mut input, mut output) = conn.into_split();
    // A single reader task owns the stream, so a select! cancellation cannot
    // swallow half an acknowledgement frame.
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<Value>(8);
    tokio::spawn(async move {
        while let Ok(Some(message)) = read_message(&mut input).await {
            if incoming_tx.send(message).await.is_err() { break; }
        }
    });
    let first = incoming_rx.recv().await.ok_or(io::ErrorKind::UnexpectedEof)?;
    if first.as_object().is_none_or(|obj| obj.len() != 3)
        || first.get("type").and_then(Value::as_str) != Some("arm")
        || first.get("protocol").and_then(Value::as_u64) != Some(1) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad arm"));
    }
    let session_id = first.get("session_id").and_then(Value::as_str)
        .filter(|id| id.len() == 36 && id.is_ascii())
        .ok_or(io::ErrorKind::InvalidData)?.to_string();
    let (send, mut requests) = mpsc::channel::<Outbound>(8);
    let (token, mut revoked) = hub.register(session_id.clone(), send).await;
    let mut sequence: u64 = 0;
    loop {
        tokio::select! {
            changed = revoked.changed() => {
                if changed.is_err() || *revoked.borrow() { break; }
            }
            msg = incoming_rx.recv() => {
                // No unsolicited messages from the extension; an ack must
                // correspond to the one outstanding commit below.
                let _ = msg;
                break;
            }
            msg = requests.recv() => {
                let Some(outbound) = msg else { break; };
                let request = json!({"type":"commit", "session_id":session_id, "sequence":sequence, "text":outbound.text});
                output.write_all(request.to_string().as_bytes()).await?;
                output.write_all(b"\n").await?;
                let reply = tokio::time::timeout(ACK_TIMEOUT, incoming_rx.recv()).await;
                let ok = matches!(reply, Ok(Some(ref ack))
                    if ack.get("type").and_then(Value::as_str) == Some("commit-result")
                    && ack.get("protocol").and_then(Value::as_u64) == Some(1)
                    && ack.get("session_id").and_then(Value::as_str) == Some(session_id.as_str())
                    && ack.get("sequence").and_then(Value::as_u64) == Some(sequence)
                    && ack.get("ok").and_then(Value::as_bool) == Some(true));
                let _ = outbound.result.send(ok);
                if !ok { break; }
                sequence += 1;
            }
        }
    }
    hub.invalidate(token).await;
    Ok(())
}

#[derive(Debug)]
pub struct FirefoxTarget {
    token: u64,
    hub: FirefoxHub,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Receiver<bool>,
}

#[async_trait]
impl Target for FirefoxTarget {
    async fn commit(&mut self, text: &str) -> Result<(), InjectError> {
        if *self.revoke.borrow() { return Err(InjectError::FocusLost); }
        // Long stable transcripts are sent as bounded, ordered Unicode-safe
        // segments. Never truncate or split a UTF-8 code point.
        for segment in segments(text) {
            let (result, answer) = oneshot::channel();
            self.send.send(Outbound { text: segment.to_owned(), result }).await
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
        // Keep the browser field armed for the next utterance. A click on the
        // extension button or document navigation revokes it explicitly.
    }
}

/// Prefer an explicitly armed Firefox field; preserve existing IBus behavior
/// for every other activation, without moving OS focus.
pub struct FirefoxOrIbusInjector<I> {
    hub: FirefoxHub,
    fallback: I,
}

impl<I> FirefoxOrIbusInjector<I> {
    pub fn new(hub: FirefoxHub, fallback: I) -> Self { Self { hub, fallback } }
}

#[async_trait]
impl<I: Injector> Injector for FirefoxOrIbusInjector<I> {
    async fn acquire(&mut self) -> Result<Box<dyn Target>, InjectError> {
        if let Some(target) = self.hub.acquire().await { return Ok(Box::new(target)); }
        self.fallback.acquire().await
    }
    fn capabilities(&self) -> TextInputCapabilities { self.fallback.capabilities() }
    fn activation(&self) -> Activation { self.fallback.activation() }
}

impl std::fmt::Debug for FirefoxHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("FirefoxHub") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_new_arm_revokes_the_previous_target() {
        let hub = FirefoxHub::default();
        let (tx, _rx) = mpsc::channel(2);
        let _ = hub.register("first".into(), tx).await;
        let mut first = hub.acquire().await.expect("armed");
        let (tx2, _rx2) = mpsc::channel(2);
        let _ = hub.register("second".into(), tx2).await;
        assert!(matches!(first.commit("hello").await, Err(InjectError::FocusLost)));
    }

    #[tokio::test]
    async fn no_implicit_target_without_explicit_arm() {
        assert!(FirefoxHub::default().acquire().await.is_none());
    }

    #[test]
    fn long_commits_are_chunked_without_bisecting_codepoints() {
        let text = "é😀".repeat(2000);
        let pieces: Vec<_> = segments(&text).collect();
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|piece| piece.len() <= MAX_COMMIT_BYTES));
        assert_eq!(pieces.join(""), text);
    }
}
