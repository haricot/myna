//! Firefox Native Messaging <-> Myna user-socket relay. No page reads and no
//! commands. Firefox launches one process per explicit extension connection.
//! Only `--install` writes a manifest; normal operation never touches disk.

use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;

use native_messaging::host::{decode_message, encode_message};
use native_messaging::{install, remove, Scope};
use serde_json::Value;

const HOST: &str = "myna.firefox_anchor";
const EXTENSION: &str = "firefox-anchor@myna.local";
const MAX_BROWSER_FRAME: usize = 8 * 1024;
const MAX_DAEMON_FRAME: usize = 64 * 1024;

fn socket_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("MYNA_FIREFOX_ANCHOR_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is required"))?;
    Ok(PathBuf::from(runtime).join("myna-firefox-anchor.sock"))
}

fn session_id(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 36
            && s.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
    })
}

fn valid_browser_message(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    match object.get("type").and_then(Value::as_str) {
        Some("ready") | Some("disarm") => {
            object.len() == 2 && object.get("protocol").and_then(Value::as_u64) == Some(1)
        }
        Some("prepare-result") => {
            object.len() == 3 && object.get("protocol").and_then(Value::as_u64) == Some(1)
                && object.get("ok").and_then(Value::as_bool).is_some()
        }
        Some("arm") => {
            object.len() == 3
                && object.get("protocol").and_then(Value::as_u64) == Some(1)
                && object.get("session_id").is_some_and(session_id)
        }
        Some("commit-result") => {
            object.len() == 5
                && object.get("session_id").is_some_and(session_id)
                && object.get("sequence").and_then(Value::as_u64).is_some()
                && object.get("ok").and_then(Value::as_bool).is_some()
                && object.get("protocol").and_then(Value::as_u64) == Some(1)
        }
        _ => false,
    }
}

fn valid_daemon_message(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    match object.get("type").and_then(Value::as_str) {
        Some("prepare") | Some("prepare-cancel") => object.len() == 2
            && object.get("protocol").and_then(Value::as_u64) == Some(1),
        Some("commit") => object.len() == 4
            && object.get("session_id").is_some_and(session_id)
            && object.get("sequence").and_then(Value::as_u64).is_some()
            && object.get("text").and_then(Value::as_str)
                .is_some_and(|t| t.len() <= 4096),
        _ => false,
    }
}

fn bounded_line(reader: &mut impl BufRead, output: &mut Vec<u8>) -> io::Result<usize> {
    output.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() { return Ok(output.len()); }
        let count = available.iter().position(|byte| *byte == b'\n')
            .map_or(available.len(), |offset| offset + 1);
        if output.len() + count > MAX_DAEMON_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "daemon frame too large"));
        }
        let done = available[count - 1] == b'\n';
        output.extend_from_slice(&available[..count]);
        reader.consume(count);
        if done { return Ok(output.len()); }
    }
}

fn relay() -> io::Result<()> {
    let stream = UnixStream::connect(socket_path()?)?;
    let mut to_daemon = stream.try_clone()?;
    let reader = thread::spawn(move || -> io::Result<()> {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let raw = match decode_message(&mut stdin, MAX_BROWSER_FRAME) {
                Ok(raw) => raw,
                Err(_) => break,
            };
            let value: Value = serde_json::from_str(&raw).map_err(io::Error::other)?;
            if !valid_browser_message(&value) {
                break;
            }
            // Forward only one validated JSON object, not raw newlines.
            to_daemon.write_all(value.to_string().as_bytes())?;
            to_daemon.write_all(b"\n")?;
        }
        to_daemon.shutdown(Shutdown::Write)
    });

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut from_daemon = BufReader::new(stream);
    let mut line = Vec::with_capacity(256);
    loop {
        // Limit even the local peer; a malicious or buggy daemon must never
        // make this process allocate unbounded memory.
        let bytes = bounded_line(&mut from_daemon, &mut line)?;
        if bytes == 0 { break; }
        if bytes > MAX_DAEMON_FRAME || !line.ends_with(b"\n") { break; }
        let value: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
        if !valid_daemon_message(&value) { break; }
        let framed = encode_message(&value).map_err(io::Error::other)?;
        stdout.write_all(&framed)?;
        stdout.flush()?;
    }
    // Closing the socket also wakes the thread waiting on stdin once Firefox
    // disconnects; avoid joining it while Firefox still holds stdin open.
    drop(reader);
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.first().and_then(|a| a.to_str()) {
        Some("--install") if args.len() == 1 => (|| -> io::Result<()> {
            let executable = std::env::current_exe()?;
            install(HOST, "Myna Firefox field anchor", &executable, &[],
                &[EXTENSION.to_string()], &["firefox"], Scope::User)
        })(),
        Some("--uninstall") if args.len() == 1 => {
            remove(HOST, &["firefox"], Scope::User)
        }
        None => relay(), // local stdio testing
        // Firefox passes the host-manifest path and the invoking extension ID
        // as its two argv values. This is a useful sanity check, not a
        // cryptographic proof of browser identity (same-user malware can
        // forge argv and replace user-level manifests).
        Some(manifest) if args.len() == 2
            && std::path::Path::new(manifest).file_name()
                == Some(std::ffi::OsStr::new("myna.firefox_anchor.json"))
            && args[1].as_os_str() == std::ffi::OsStr::new(EXTENSION) => relay(),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown argument")),
    };
    if let Err(err) = result {
        eprintln!("myna-firefox-host: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_only_the_minimal_firefox_protocol() {
        let id = "badc0ffe-1234-4321-abcd-000000000000";
        assert!(valid_browser_message(&json!({"type":"ready","protocol":1})));
        assert!(valid_browser_message(&json!({"type":"disarm","protocol":1})));
        assert!(valid_browser_message(&json!({"type":"prepare-result","protocol":1,"ok":false})));
        assert!(!valid_browser_message(&json!({"type":"prepare-result","protocol":1,"ok":true,"text":"secret"})));
        assert!(valid_browser_message(&json!({"type":"arm","protocol":1,"session_id":id})));
        assert!(valid_browser_message(&json!({"type":"commit-result","protocol":1,"session_id":id,"sequence":0,"ok":true})));
        assert!(!valid_browser_message(&json!({"type":"arm","protocol":1,"session_id":id,"text":"secret"})));
        assert!(!valid_browser_message(&json!({"type":"run","command":"sh"})));
        assert!(!valid_browser_message(&json!({"type":"commit-result","session_id":id,"sequence":0,"ok":true})));
    }

    #[test]
    fn daemon_may_send_only_bounded_committed_text() {
        let id = "badc0ffe-1234-4321-abcd-000000000000";
        assert!(valid_daemon_message(&json!({"type":"prepare","protocol":1})));
        assert!(valid_daemon_message(&json!({"type":"prepare-cancel","protocol":1})));
        assert!(!valid_daemon_message(&json!({"type":"prepare","protocol":1,"command":"exec"})));
        assert!(valid_daemon_message(&json!({"type":"commit","session_id":id,"sequence":0,"text":"ok"})));
        assert!(!valid_daemon_message(&json!({"type":"commit","session_id":id,"sequence":0,"text":"x".repeat(5000)})));
        assert!(!valid_daemon_message(&json!({"type":"eval","code":"..."})));
    }
}
