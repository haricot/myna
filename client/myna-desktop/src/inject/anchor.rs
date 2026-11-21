// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in GNOME window anchoring, scoped to one acquired target.
//!
//! The Shell helper restores the window; IBus still owns the field and checks
//! secure fields. Only a restoration announced by that helper counts as a
//! focus blip. A field change inside the same window remains a focus loss.
use super::{Activation, FocusEvent, InjectError, Injector, Target, TextInputCapabilities};
use async_trait::async_trait;
use futures_util::stream::{self, BoxStream, StreamExt};
use std::time::Duration;
use tokio::{sync::watch, task::JoinHandle, time::Instant};
use zbus::{Connection, Proxy};

const SERVICE: &str = "org.haricot.Myna.FocusAnchor";
const PATH: &str = "/org/haricot/Myna/FocusAnchor";
const CALL_WAIT: Duration = Duration::from_millis(500);
const RESTORE_WAIT: Duration = Duration::from_secs(1);

pub struct FocusAnchor<I> {
    inner: I,
    enabled: bool,
}

impl<I> FocusAnchor<I> {
    pub fn new(inner: I, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

#[async_trait]
impl<I: Injector> Injector for FocusAnchor<I> {
    async fn acquire(&mut self) -> Result<Box<dyn Target>, InjectError> {
        if !self.enabled {
            return self.inner.acquire().await;
        }
        // Begin before IBus acquisition and before any microphone is opened.
        let anchor = WindowAnchor::begin(self.inner.activation()).await?;
        let target = self.inner.acquire().await?;
        if let Err(error) = anchor.ready().await {
            target.release().await;
            return Err(error);
        }
        myna_core::dbg_log!("anchor", "window anchored for this dictation");
        Ok(Box::new(AnchoredTarget { target, anchor }))
    }

    fn capabilities(&self) -> TextInputCapabilities {
        self.inner.capabilities()
    }

    fn activation(&self) -> Activation {
        self.inner.activation()
    }
}

#[derive(Debug)]
struct AnchoredTarget {
    target: Box<dyn Target>,
    anchor: WindowAnchor,
}

#[async_trait]
impl Target for AnchoredTarget {
    async fn commit(&mut self, text: &str) -> Result<(), InjectError> {
        // Never ask IBus to write while the original window is away.
        self.anchor.ready().await?;
        self.target.commit(text).await
    }

    async fn set_preedit(&mut self, text: &str) {
        if self.anchor.ready().await.is_ok() {
            self.target.set_preedit(text).await;
        }
    }

    fn char_before_cursor(&self) -> Option<char> {
        self.target.char_before_cursor()
    }

    fn focus_events(&self) -> BoxStream<'static, FocusEvent> {
        let mut lost = self.anchor.lost.clone();
        let anchor_loss = stream::once(async move {
            let _ = lost.wait_for(|&lost| lost).await;
            FocusEvent::TargetGone
        });
        stream::select(self.target.focus_events(), anchor_loss).boxed()
    }

    fn activated(&self) {
        self.target.activated();
    }

    async fn release(self: Box<Self>) {
        let Self { target, anchor } = *self;
        // Keep the window anchored through the last IBus flush and preedit clear.
        target.release().await;
        anchor.end().await;
    }
}

/// A dedicated bus connection is also the lease's lifetime: a crash, Ctrl+C,
/// cancelled acquire or dropped target disconnects it, so Shell releases the
/// window even if the asynchronous End call could not run.
#[derive(Debug)]
struct CloseConnection(Connection);
impl Drop for CloseConnection {
    fn drop(&mut self) {
        let connection = self.0.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = connection.close().await;
            });
        }
    }
}

#[derive(Debug)]
struct WindowAnchor {
    proxy: Proxy<'static>,
    token: u32,
    lost: watch::Receiver<bool>,
    watcher: JoinHandle<()>,
    _connection: CloseConnection,
}

impl WindowAnchor {
    async fn begin(activation: Activation) -> Result<Self, InjectError> {
        let connection = Connection::session().await.map_err(unavailable)?;
        Self::on_connection(connection, activation).await
    }

    async fn on_connection(
        connection: Connection,
        activation: Activation,
    ) -> Result<Self, InjectError> {
        let connection = CloseConnection(connection);
        let proxy = Proxy::new_owned(connection.0.clone(), SERVICE, PATH, SERVICE)
            .await
            .map_err(unavailable)?;
        let mut events = proxy.receive_signal("Event").await.map_err(unavailable)?;
        let token: u32 = tokio::time::timeout(CALL_WAIT, proxy.call("Begin", &()))
            .await
            .map_err(unavailable)?
            .map_err(unavailable)?;
        let (lost_tx, lost) = watch::channel(false);
        let check = proxy.clone();
        let watcher = tokio::spawn(async move {
            let mut poll = tokio::time::interval(Duration::from_millis(250));
            let mut away_since = None;
            loop {
                tokio::select! {
                    biased;
                    event = events.next() => {
                        let Some(event) = event else { break; };
                        let Ok((id, kind)) = event.body().deserialize::<(u32, String)>() else { continue; };
                        if id != token { continue; }
                        match kind.as_str() {
                            "restoring" => {
                                activation.signal();
                                myna_core::dbg_log!("anchor", "restoring the anchored window");
                            }
                            "lost" => break,
                            _ => {}
                        }
                    }
                    _ = poll.tick() => {
                        match check_state(&check, token).await.as_deref() {
                            Some("held") => away_since = None,
                            Some("away") => {
                                if away_since.get_or_insert_with(Instant::now).elapsed() >= RESTORE_WAIT {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                }
            }
            let _ = lost_tx.send(true);
        });
        Ok(Self {
            proxy,
            token,
            lost,
            watcher,
            _connection: connection,
        })
    }

    async fn ready(&self) -> Result<(), InjectError> {
        let ready = async {
            loop {
                if *self.lost.borrow() {
                    return false;
                }
                match check_state(&self.proxy, self.token).await.as_deref() {
                    Some("held") => return true,
                    Some("away") => tokio::time::sleep(Duration::from_millis(10)).await,
                    _ => return false,
                }
            }
        };
        match tokio::time::timeout(RESTORE_WAIT, ready).await {
            Ok(true) => Ok(()),
            _ => Err(InjectError::FocusLost),
        }
    }

    async fn end(&self) {
        let _ = tokio::time::timeout(
            CALL_WAIT,
            self.proxy.call::<_, _, ()>("End", &(self.token,)),
        )
        .await;
        myna_core::dbg_log!("anchor", "window released");
    }
}

impl Drop for WindowAnchor {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

async fn check_state(proxy: &Proxy<'_>, token: u32) -> Option<String> {
    tokio::time::timeout(CALL_WAIT, proxy.call("Check", &(token,)))
        .await
        .ok()?
        .ok()
}

fn unavailable(error: impl std::fmt::Display) -> InjectError {
    InjectError::Unavailable(format!(
        "focus anchoring requires the enabled Myna Focus Anchor GNOME extension: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inject::mock::MockInjector;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    struct Bus(Child, String);
    impl Bus {
        fn new() -> Self {
            let mut process = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .expect("anchor transport tests require dbus-daemon");
            let mut address = String::new();
            BufReader::new(process.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            Self(process, address.trim().into())
        }
        async fn connect(&self) -> Connection {
            zbus::connection::Builder::address(self.1.as_str())
                .unwrap()
                .build()
                .await
                .unwrap()
        }
    }
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[derive(Default)]
    struct Shell {
        token: u32,
        state: String,
        ends: usize,
    }
    struct Service(Arc<Mutex<Shell>>);
    #[zbus::interface(name = "org.haricot.Myna.FocusAnchor")]
    impl Service {
        fn begin(&self) -> u32 {
            let mut state = self.0.lock().unwrap();
            state.token += 1;
            state.state = "held".into();
            state.token
        }
        fn check(&self, token: u32) -> String {
            let state = self.0.lock().unwrap();
            if state.token == token {
                state.state.clone()
            } else {
                "lost".into()
            }
        }
        fn end(&self, token: u32) {
            let mut state = self.0.lock().unwrap();
            assert_eq!(token, state.token);
            state.ends += 1;
            state.state = "lost".into();
        }
    }

    struct Fixture {
        server: Connection,
        shell: Arc<Mutex<Shell>>,
        _bus: Bus,
    }
    impl Fixture {
        async fn new() -> Self {
            let bus = Bus::new();
            let shell = Arc::new(Mutex::new(Shell::default()));
            let server = zbus::connection::Builder::address(bus.1.as_str())
                .unwrap()
                .name(SERVICE)
                .unwrap()
                .serve_at(PATH, Service(shell.clone()))
                .unwrap()
                .build()
                .await
                .unwrap();
            Self {
                server,
                shell,
                _bus: bus,
            }
        }
        async fn anchor(&self, activation: Activation) -> WindowAnchor {
            WindowAnchor::on_connection(self._bus.connect().await, activation)
                .await
                .unwrap()
        }
        async fn event(&self, token: u32, kind: &str) {
            self.server
                .emit_signal(None::<&str>, PATH, SERVICE, "Event", &(token, kind))
                .await
                .unwrap();
        }
    }

    #[derive(Debug, Default)]
    struct Record {
        commits: Vec<String>,
        released: bool,
        secure: bool,
        field_lost: bool,
    }
    #[derive(Debug)]
    struct Field(Arc<Mutex<Record>>);
    #[async_trait]
    impl Target for Field {
        async fn commit(&mut self, text: &str) -> Result<(), InjectError> {
            let mut record = self.0.lock().unwrap();
            if record.secure {
                return Err(InjectError::SecureField);
            }
            if record.field_lost {
                return Err(InjectError::FocusLost);
            }
            record.commits.push(text.into());
            Ok(())
        }
        fn focus_events(&self) -> BoxStream<'static, FocusEvent> {
            stream::pending().boxed()
        }
        async fn release(self: Box<Self>) {
            self.0.lock().unwrap().released = true;
        }
    }
    async fn until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn disabled_anchoring_uses_the_existing_injector_without_a_shell_service() {
        let mut injector = FocusAnchor::new(MockInjector::new(), false);
        let mut target = injector.acquire().await.unwrap();
        target.commit("hello").await.unwrap();
        target.release().await;
    }

    #[tokio::test]
    async fn a_missing_helper_refuses_anchoring_before_any_field_is_acquired() {
        let bus = Bus::new();
        let result = WindowAnchor::on_connection(bus.connect().await, Activation::none()).await;
        assert!(matches!(result, Err(InjectError::Unavailable(_))));
    }

    #[tokio::test]
    async fn restoration_waits_for_the_original_window_and_marks_only_its_own_blip() {
        let fixture = Fixture::new().await;
        let activations = Arc::new(AtomicUsize::new(0));
        let count = activations.clone();
        let anchor = fixture
            .anchor(Activation::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }))
            .await;
        let token = anchor.token;
        let record = Arc::new(Mutex::new(Record::default()));
        let mut target = AnchoredTarget {
            target: Box::new(Field(record.clone())),
            anchor,
        };
        fixture.shell.lock().unwrap().state = "away".into();
        let write = tokio::spawn(async move {
            target.commit("hello").await.unwrap();
            target
        });
        fixture.event(token + 1, "restoring").await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(record.lock().unwrap().commits.is_empty());
        assert_eq!(activations.load(Ordering::SeqCst), 0);
        fixture.event(token, "restoring").await;
        until(|| activations.load(Ordering::SeqCst) == 1).await;
        fixture.shell.lock().unwrap().state = "held".into();
        let target = write.await.unwrap();
        assert_eq!(record.lock().unwrap().commits, ["hello"]);
        Box::new(target).release().await;
        assert!(record.lock().unwrap().released);
        assert_eq!(fixture.shell.lock().unwrap().ends, 1);
    }

    #[tokio::test]
    async fn loss_of_the_extension_or_window_ends_the_target_and_prevents_insertion() {
        for lost_signal in [true, false] {
            let fixture = Fixture::new().await;
            let anchor = fixture.anchor(Activation::none()).await;
            let token = anchor.token;
            let record = Arc::new(Mutex::new(Record::default()));
            let mut target = AnchoredTarget {
                target: Box::new(Field(record.clone())),
                anchor,
            };
            let mut focus = target.focus_events();
            if lost_signal {
                fixture.event(token, "lost").await;
            } else {
                fixture.server.clone().close().await.unwrap();
            }
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), focus.next())
                    .await
                    .unwrap(),
                Some(FocusEvent::TargetGone)
            );
            assert!(matches!(
                target.commit("discard").await,
                Err(InjectError::FocusLost)
            ));
            assert!(record.lock().unwrap().commits.is_empty());
        }
    }

    #[tokio::test]
    async fn anchoring_never_bypasses_the_field_or_secure_field_guard() {
        for secure in [true, false] {
            let fixture = Fixture::new().await;
            let anchor = fixture.anchor(Activation::none()).await;
            let record = Arc::new(Mutex::new(Record {
                secure,
                field_lost: !secure,
                ..Default::default()
            }));
            let mut target = AnchoredTarget {
                target: Box::new(Field(record.clone())),
                anchor,
            };
            let error = target.commit("discard").await.unwrap_err();
            assert!(matches!(
                (secure, error),
                (true, InjectError::SecureField) | (false, InjectError::FocusLost)
            ));
            assert!(record.lock().unwrap().commits.is_empty());
        }
    }

    #[tokio::test]
    async fn dropping_a_target_disconnects_its_private_bus_owner_without_an_end_call() {
        let fixture = Fixture::new().await;
        let anchor = fixture.anchor(Activation::none()).await;
        let owner = anchor.proxy.connection().unique_name().unwrap().to_string();
        let daemon = Proxy::new(
            &fixture.server,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .unwrap();
        drop(anchor);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let exists: bool = daemon
                    .call("NameHasOwner", &(owner.as_str(),))
                    .await
                    .unwrap();
                if !exists {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fixture.shell.lock().unwrap().ends, 0);
    }
}
