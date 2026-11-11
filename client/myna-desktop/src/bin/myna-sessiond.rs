//! Experimental, IBus-free session owner for native Firefox and egui fields.
//!
//! Reuses myna-testbed's PipeWire -> myna-orchestrator -> IE115 pipeline and
//! the existing multi-utterance controller. Exactly one process owns capture;
//! apps receive text only through their explicit, revocable anchors.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use async_trait::async_trait;
use myna_audio::{CaptureSource, PipeWireBackend};
use myna_core::{AudioFormat, SessionConfig};
use myna_desktop::controller::{ChannelSink, SessionRun};
use myna_desktop::inject::egui::{socket_path as egui_path, EguiHub, EguiOrInjector};
use myna_desktop::inject::firefox::{socket_path as firefox_path, FirefoxHub, FirefoxOrIbusInjector};
use myna_desktop::inject::{InjectError, Injector, Target, TextInputCapabilities};
use myna_desktop::shortcut::control::{default_socket_path, send_toggle, ControlTrigger};
use myna_desktop::{DesktopController, Indicator, IndicatorState, Session};
use myna_orchestrator::{run_dictation, WsUnixIe115Backend};
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;

const USAGE: &str = "\
myna-sessiond - one microphone/session owner for native application clients

    myna-sessiond --socket <provider.sock> [--language fr] [--target <node>]
    myna-sessiond --toggle

Firefox and egui keep their existing native anchor protocols. The service
does not open IBus and refuses to start when the Myna control socket exists.
Stop myna-desktop before switching to this experimental service.\n";

#[derive(Debug)]
struct Options {
    socket: PathBuf,
    language: Option<String>,
    target: Option<String>,
}

fn parse() -> Result<Option<Options>, String> {
    let mut socket = None;
    let mut language = None;
    let mut target = None;
    let mut toggle = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or("--socket needs a path")?)),
            "--language" => language = Some(args.next().ok_or("--language needs a value")?),
            "--target" => target = Some(args.next().ok_or("--target needs a PipeWire node")?),
            "--toggle" => toggle = true,
            "--help" | "-h" => { println!("{USAGE}"); return Ok(None); }
            other => return Err(format!("unknown argument: {other}\n{USAGE}")),
        }
    }
    if toggle {
        if socket.is_some() || language.is_some() || target.is_some() {
            return Err("--toggle cannot be combined with daemon options".into());
        }
        return Ok(None);
    }
    Ok(Some(Options { socket: socket.ok_or(format!("--socket is required\n{USAGE}"))?, language, target }))
}

/// No global text injection. An unarmed native app must never cause capture.
struct NoFallback;

#[async_trait]
impl Injector for NoFallback {
    async fn acquire(&mut self) -> Result<Box<dyn Target>, InjectError> {
        Err(InjectError::NoTarget)
    }

    fn capabilities(&self) -> TextInputCapabilities {
        // The egui target supports volatile preedit. Firefox ignores it.
        TextInputCapabilities { preedit: true, ..TextInputCapabilities::COMMIT_ONLY }
    }
}

/// Logs state only, never microphone samples or dictated text.
struct Status;

#[async_trait]
impl Indicator for Status {
    async fn set_state(&mut self, state: IndicatorState) {
        match state {
            IndicatorState::Hidden => eprintln!("myna-sessiond: idle"),
            IndicatorState::Recording => eprintln!("myna-sessiond: recording"),
            IndicatorState::Transcribing | IndicatorState::Finalizing => eprintln!("myna-sessiond: finishing"),
            IndicatorState::Error { message, .. } => eprintln!("myna-sessiond: {message}"),
        }
    }
}

/// The live user runtime directory must be private and not a symlink. No
/// /tmp fallback or unlink-on-bind: two daemons must not steal one socket.
fn private_runtime(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != std::fs::metadata("/proc/self")?.uid()
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be owned by this user, private and not a symlink"));
    }
    Ok(())
}

fn control_listener(path: &Path) -> io::Result<UnixListener> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    private_runtime(parent)?;
    let listener = UnixListener::bind(path)?; // EADDRINUSE is intentional.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

struct SocketGuard {
    path: PathBuf,
    dev: u64,
    inode: u64,
}

impl SocketGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self { path, dev: metadata.dev(), inode: metadata.ino() })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|now| now.dev() == self.dev && now.ino() == self.inode && now.file_type().is_socket())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn control_loop(listener: UnixListener, poke: myna_desktop::shortcut::control::Poke, uid: u32) {
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    while let Ok((mut conn, _)) = listener.accept().await {
        if !conn.peer_cred().is_ok_and(|cred| cred.uid() == uid) { continue; }
        let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
        tokio::spawn({
            let poke = poke.clone();
            async move {
                let _permit = permit;
                let mut payload = [0u8; 7];
                // Refuse commands, arbitrary payloads and stalled connections.
                let authorized = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    conn.read_exact(&mut payload[..6]).await?;
                    let trailing = conn.read(&mut payload[6..]).await?;
                    Ok::<bool, io::Error>(trailing == 0 && &payload[..6] == b"toggle")
                }).await;
                if matches!(authorized, Ok(Ok(true))) {
                    poke.poke();
                }
            }
        });
    }
}

async fn serve(options: Options) -> io::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or(io::ErrorKind::NotFound)?;
    let runtime = PathBuf::from(runtime);
    private_runtime(&runtime)?;
    let uid = std::fs::metadata(&runtime)?.uid();
    let control_path = default_socket_path();
    let listener = control_listener(&control_path)?;
    let _control_guard = SocketGuard::new(control_path.clone())?;

    let firefox = FirefoxHub::default();
    let egui = EguiHub::default();
    let browser_socket = firefox_path()?;
    let egui_socket = egui_path()?;
    // Existing hosts and clients continue to speak their original protocols.
    // The hub bind operations reject pre-existing sockets, rather than
    // replacing another process's listeners.
    let browser = tokio::spawn({ let hub = firefox.clone(); async move { hub.serve(browser_socket).await } });
    let native = tokio::spawn({ let hub = egui.clone(); async move { hub.serve(egui_socket).await } });

    let mut trigger = ControlTrigger::new();
    let injector = EguiOrInjector::new(egui, FirefoxOrIbusInjector::new(firefox, NoFallback));
    trigger.hear(injector.activation());
    let poke = trigger.poke();

    let mut controller = DesktopController::builder()
        .trigger(trigger)
        .injector(injector)
        .indicator(Status)
        .preedit(true)
        .session(move |events| {
            let mut capture = CaptureSource::builder(AudioFormat::default());
            if let Some(node) = &options.target { capture = capture.target(node.clone()); }
            let source = capture.backend(Box::new(PipeWireBackend::new())).build();
            let stop = source.stop_handle();
            let stats = source.stats();
            let socket = options.socket.clone();
            let language = options.language.clone();
            let run: SessionRun = Box::pin(async move {
                let backend = WsUnixIe115Backend::new(socket);
                let mut sink = ChannelSink(events);
                run_dictation(&backend, SessionConfig { language, ..Default::default() }, source, &mut sink).await
            });
            Session { run, stop, stats: Some(stats) }
        })
        .build();

    eprintln!("myna-sessiond: ready on {} (no IBus, one capture owner)", control_path.display());
    tokio::select! {
        () = control_loop(listener, poke, uid) => Err(io::Error::other("control socket closed")),
        () = controller.run() => Ok(()),
        outcome = browser => Err(io::Error::other(format!("Firefox anchor ended: {outcome:?}"))),
        outcome = native => Err(io::Error::other(format!("egui anchor ended: {outcome:?}"))),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let is_toggle = std::env::args().any(|arg| arg == "--toggle");
    let result = parse();
    if is_toggle && matches!(result, Ok(None)) {
        return match send_toggle(default_socket_path()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => { eprintln!("myna-sessiond: toggle failed: {err}"); ExitCode::FAILURE }
        };
    }
    let result = match result {
        Ok(Some(options)) => serve(options).await.map_err(|err| err.to_string()),
        Ok(None) => return ExitCode::SUCCESS,
        Err(err) => Err(err),
    };
    if let Err(err) = result { eprintln!("myna-sessiond: {err}"); return ExitCode::FAILURE; }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fallback_requires_an_explicit_target() {
        let mut fallback = NoFallback;
        assert!(matches!(tokio::runtime::Builder::new_current_thread().enable_all().build()
            .unwrap().block_on(fallback.acquire()), Err(InjectError::NoTarget)));
    }

    #[tokio::test]
    async fn neither_browser_nor_egui_can_capture_without_an_anchor() {
        let mut injector = EguiOrInjector::new(
            EguiHub::default(),
            FirefoxOrIbusInjector::new(FirefoxHub::default(), NoFallback),
        );
        assert!(injector.capabilities().preedit);
        assert!(matches!(injector.acquire().await, Err(InjectError::NoTarget)));
    }

    #[tokio::test]
    async fn one_control_socket_owner_and_no_stolen_path() {
        let root = std::env::temp_dir().join(format!("myna-sessiond-{}-{}", std::process::id(), "exclusive"));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("control.sock");
        let listener = control_listener(&path).unwrap();
        assert_eq!(control_listener(&path).unwrap_err().kind(), io::ErrorKind::AddrInUse);
        let guard = SocketGuard::new(path.clone()).unwrap();
        drop(listener);
        drop(guard);
        assert!(!path.exists());
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn control_rejects_unknown_commands() {
        use tokio::io::AsyncWriteExt;
        use myna_orchestrator::Trigger as _;
        let root = std::env::temp_dir().join(format!("myna-sessiond-{}-{}", std::process::id(), "toggle"));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("control.sock");
        let listener = control_listener(&path).unwrap();
        let _guard = SocketGuard::new(path.clone()).unwrap();
        let mut trigger = ControlTrigger::new();
        let task = tokio::spawn(control_loop(listener, trigger.poke(), std::fs::metadata(&root).unwrap().uid()));
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client.write_all(b"unsafe").await.unwrap();
        drop(client);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), trigger.next_edge()).await.is_err());
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client.write_all(b"toggle").await.unwrap();
        drop(client);
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(1), trigger.next_edge()).await.unwrap(),
            Some(myna_orchestrator::TriggerEdge::Press));
        task.abort();
        // The guard still owns the path until the end of this scope.
        drop(_guard);
        std::fs::remove_dir(root).unwrap();
    }
}
