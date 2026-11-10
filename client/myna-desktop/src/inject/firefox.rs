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
const PREPARE_TIMEOUT: Duration = Duration::from_millis(750);
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

enum Outbound {
    Prepare(oneshot::Sender<bool>),
    Commit {
        token: u64,
        text: String,
        result: oneshot::Sender<bool>,
    },
}

struct Armed {
    token: u64,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Sender<bool>,
    target: Option<watch::Sender<bool>>,
}

struct Ready {
    token: u64,
    send: mpsc::Sender<Outbound>,
    revoke: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    next_token: u64,
    armed: Option<Armed>,
    ready: Option<Ready>,
}

#[derive(Clone, Default)]
pub struct FirefoxHub(Arc<Mutex<State>>);

impl FirefoxHub {
    /// A Firefox background script maintains this idle connection independently
    /// from its current page anchor. The daemon's GNOME shortcut needs no
    /// knowledge of any Firefox keyboard shortcut.
    async fn register_ready(&self, send: mpsc::Sender<Outbound>) -> (u64, watch::Receiver<bool>) {
        let mut state = self.0.lock().await;
        if let Some(old) = state.ready.take() { let _ = old.revoke.send(true); }
        if let Some(old) = state.armed.take() {
            let _ = old.revoke.send(true);
            if let Some(target) = old.target { let _ = target.send(true); }
        }
        state.next_token += 1;
        let token = state.next_token;
        let (revoke, receiver) = watch::channel(false);
        state.ready = Some(Ready { token, send, revoke });
        (token, receiver)
    }

    async fn unregister_ready(&self, token: u64) {
        let mut state = self.0.lock().await;
        if state.ready.as_ref().is_some_and(|r| r.token == token) {
            state.ready.take();
            if let Some(old) = state.armed.take() {
                let _ = old.revoke.send(true);
                if let Some(target) = old.target { let _ = target.send(true); }
            }
        }
    }

    /// Only a connected and permissioned browser can provide an anchor.
    /// Failure or timeout leaves Myna's existing IBus path intact.
    async fn prepare(&self) -> bool {
        let sender = { self.0.lock().await.ready.as_ref().map(|ready| ready.send.clone()) };
        let Some(sender) = sender else { return false; };
        let (result, answer) = oneshot::channel();
        if sender.send(Outbound::Prepare(result)).await.is_err() { return false; }
        matches!(tokio::time::timeout(PREPARE_TIMEOUT, answer).await, Ok(Ok(true)))
    }

    async fn register(&self, send: mpsc::Sender<Outbound>) -> (u64, watch::Receiver<bool>) {
        let mut state = self.0.lock().await;
        if let Some(old) = state.armed.take() {
            let _ = old.revoke.send(true);
            if let Some(target) = old.target { let _ = target.send(true); }
        }
        state.next_token += 1;
        let token = state.next_token;
        let (revoke, receiver) = watch::channel(false);
        state.armed = Some(Armed { token, send, revoke, target: None });
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

async fn write_message(output: &mut tokio::net::unix::OwnedWriteHalf, message: Value) -> io::Result<()> {
    output.write_all(message.to_string().as_bytes()).await?;
    output.write_all(b"\n").await
}

struct PendingCommit {
    token: u64,
    session_id: String,
    sequence: u64,
    result: oneshot::Sender<bool>,
    deadline: tokio::time::Instant,
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
    let (send, mut requests) = mpsc::channel::<Outbound>(8);
    let ready = first.get("type").and_then(Value::as_str) == Some("ready")
        && first.as_object().is_some_and(|obj| obj.len() == 2)
        && first.get("protocol").and_then(Value::as_u64) == Some(1);
    let old_arm = first.get("type").and_then(Value::as_str) == Some("arm")
        && first.as_object().is_some_and(|obj| obj.len() == 3)
        && first.get("protocol").and_then(Value::as_u64) == Some(1)
        && first.get("session_id").and_then(Value::as_str)
            .is_some_and(|id| id.len() == 36 && id.is_ascii());
    if !ready && !old_arm {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad Firefox handshake"));
    }
    let (connection_token, mut revoked) = if ready {
        hub.register_ready(send.clone()).await
    } else {
        hub.register(send.clone()).await
    };
    let mut armed_token = if old_arm { Some(connection_token) } else { None };
    let mut session_id = first.get("session_id").and_then(Value::as_str).map(str::to_owned);
    let mut pending_prepare: Option<oneshot::Sender<bool>> = None;
    let mut pending_commit: Option<PendingCommit> = None;
    let mut sequence: u64 = 0;
    loop {
        tokio::select! {
            changed = revoked.changed() => {
                if changed.is_err() || *revoked.borrow() { break; }
            }
            msg = incoming_rx.recv() => {
                let Some(msg) = msg else { break; };
                let well_formed = msg.as_object().is_some();
                if !well_formed || msg.get("protocol").and_then(Value::as_u64) != Some(1) { break; }
                match msg.get("type").and_then(Value::as_str) {
                    Some("arm") if ready && msg.as_object().is_some_and(|obj| obj.len() == 3)
                        && pending_commit.is_none() => {
                        // The controller has already fallen back to IBus if
                        // this preparation's waiting receiver timed out.
                        if pending_prepare.as_ref().is_some_and(oneshot::Sender::is_closed) {
                            write_message(&mut output, json!({"type":"prepare-cancel","protocol":1})).await?;
                            continue;
                        }
                        let Some(id) = msg.get("session_id").and_then(Value::as_str)
                            .filter(|id| id.len() == 36 && id.is_ascii()) else { break; };
                        let (token, _) = hub.register(send.clone()).await;
                        armed_token = Some(token);
                        session_id = Some(id.to_owned());
                        sequence = 0;
                    }
                    Some("disarm") if ready && msg.as_object().is_some_and(|obj| obj.len() == 2) => {
                        if let Some(token) = armed_token.take() { hub.invalidate(token).await; }
                        session_id = None;
                        if let Some(answer) = pending_prepare.take() { let _ = answer.send(false); }
                    }
                    Some("prepare-result") if ready && msg.as_object().is_some_and(|obj| obj.len() == 3) => {
                        let Some(answer) = pending_prepare.take() else { break; };
                        let Some(ok) = msg.get("ok").and_then(Value::as_bool) else { break; };
                        let accepted = ok && armed_token.is_some() && !answer.is_closed();
                        if !accepted && ok {
                            if let Some(token) = armed_token.take() { hub.invalidate(token).await; }
                            session_id = None;
                            write_message(&mut output, json!({"type":"prepare-cancel","protocol":1})).await?;
                        }
                        let _ = answer.send(accepted);
                    }
                    Some("commit-result") if msg.as_object().is_some_and(|obj| obj.len() == 5) => {
                        let Some(waiting) = pending_commit.take() else { break; };
                        let ok = msg.get("session_id").and_then(Value::as_str) == Some(waiting.session_id.as_str())
                            && msg.get("sequence").and_then(Value::as_u64) == Some(waiting.sequence)
                            && msg.get("ok").and_then(Value::as_bool) == Some(true)
                            && armed_token == Some(waiting.token);
                        let _ = waiting.result.send(ok);
                        if !ok {
                            hub.invalidate(waiting.token).await;
                            armed_token = None;
                            session_id = None;
                        } else {
                            sequence += 1;
                        }
                    }
                    _ => break,
                }
            }
            msg = requests.recv(), if pending_commit.is_none() => {
                let Some(outbound) = msg else { break; };
                match outbound {
                    Outbound::Prepare(answer) if ready => {
                        if let Some(previous) = pending_prepare.replace(answer) { let _ = previous.send(false); }
                        write_message(&mut output, json!({"type":"prepare","protocol":1})).await?;
                    }
                    Outbound::Prepare(answer) => { let _ = answer.send(false); }
                    Outbound::Commit { token, text, result } => {
                        if armed_token != Some(token) || session_id.is_none() {
                            let _ = result.send(false);
                            continue;
                        }
                        let id = session_id.clone().expect("validated above");
                        write_message(&mut output, json!({"type":"commit", "session_id":id, "sequence":sequence, "text":text})).await?;
                        pending_commit = Some(PendingCommit {
                            token, session_id: id, sequence, result,
                            deadline: tokio::time::Instant::now() + ACK_TIMEOUT,
                        });
                    }
                }
            }
            _ = tokio::time::sleep_until(pending_commit.as_ref().map_or_else(tokio::time::Instant::now, |p| p.deadline)), if pending_commit.is_some() => {
                break;
            }
        }
    }
    if let Some(token) = armed_token { hub.invalidate(token).await; }
    if ready { hub.unregister_ready(connection_token).await; }
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
            self.send.send(Outbound::Commit { token: self.token, text: segment.to_owned(), result }).await
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
        // A new GNOME activation renews the Firefox anchor at the caret's
        // *current* location when Firefox is foreground and authorized.
        if self.hub.prepare().await {
            if let Some(target) = self.hub.acquire().await { return Ok(Box::new(target)); }
        }
        // Preserve the manually armed lease if the browser is no longer
        // foreground (the focus-independent dictation use case).
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
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test]
    async fn a_new_arm_revokes_the_previous_target() {
        let hub = FirefoxHub::default();
        let (tx, _rx) = mpsc::channel(2);
        let _ = hub.register(tx).await;
        let mut first = hub.acquire().await.expect("armed");
        let (tx2, _rx2) = mpsc::channel(2);
        let _ = hub.register(tx2).await;
        assert!(matches!(first.commit("hello").await, Err(InjectError::FocusLost)));
    }

    #[tokio::test]
    async fn no_implicit_target_without_explicit_arm() {
        assert!(FirefoxHub::default().acquire().await.is_none());
    }

    #[tokio::test]
    async fn preparing_without_a_browser_is_an_immediate_fallback() {
        assert!(!FirefoxHub::default().prepare().await);
    }

    #[tokio::test]
    async fn unified_toggle_prepares_before_a_firefox_commit() {
        let hub = FirefoxHub::default();
        let (daemon, browser) = UnixStream::pair().unwrap();
        let task = tokio::spawn(handle_connection(hub.clone(), daemon));
        let (browser_read, mut browser_write) = browser.into_split();
        let mut reader = BufReader::new(browser_read);
        browser_write.write_all(b"{\"type\":\"ready\",\"protocol\":1}\n").await.unwrap();

        // Wait for the ready socket rather than racing the daemon's reader.
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if hub.0.lock().await.ready.is_some() { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();

        let preparer = tokio::spawn({ let hub = hub.clone(); async move { hub.prepare().await } });
        let mut command = String::new();
        reader.read_line(&mut command).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&command).unwrap(),
            json!({"type":"prepare","protocol":1}));

        let id = "badc0ffe-1234-4321-abcd-000000000000";
        browser_write.write_all(format!("{{\"type\":\"arm\",\"protocol\":1,\"session_id\":\"{id}\"}}\n").as_bytes()).await.unwrap();
        browser_write.write_all(b"{\"type\":\"prepare-result\",\"protocol\":1,\"ok\":true}\n").await.unwrap();
        assert!(preparer.await.unwrap());

        let mut target = hub.acquire().await.expect("prepared browser anchor");
        let commit = tokio::spawn(async move { target.commit("bonjour").await });
        command.clear();
        reader.read_line(&mut command).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&command).unwrap(),
            json!({"type":"commit","session_id":id,"sequence":0,"text":"bonjour"}));
        browser_write.write_all(format!("{{\"type\":\"commit-result\",\"protocol\":1,\"session_id\":\"{id}\",\"sequence\":0,\"ok\":true}}\n").as_bytes()).await.unwrap();
        assert!(commit.await.unwrap().is_ok());
        drop(browser_write);
        task.await.unwrap().unwrap();
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
