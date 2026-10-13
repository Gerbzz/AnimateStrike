//! Opt-in, authenticated JSON-lines control for the running native editor.
//! Only loopback can connect; all document operations run on the UI thread.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MAX_LINE: usize = 64 * 1024;
const MAX_REPLY: usize = 128 * 1024;
const MAX_CONNECTIONS: usize = 4;
const DEADLINE: Duration = Duration::from_secs(15);

pub struct Request {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
    pub deadline: Instant,
}

#[derive(Clone)]
pub struct Roots {
    read: PathBuf,
    write: PathBuf,
}

impl Roots {
    pub fn new(read: &Path, write: &Path) -> Result<Self, String> {
        fn root(path: &Path) -> Result<PathBuf, String> {
            if !path.is_absolute() {
                return Err("control roots must be absolute paths".into());
            }
            let canonical = path.canonicalize().map_err(|e| format!("cannot resolve control root: {e}"))?;
            if !canonical.is_dir() || canonical.parent().is_none() {
                return Err("control roots must be existing subdirectories".into());
            }
            Ok(canonical)
        }
        Ok(Self { read: root(read)?, write: root(write)? })
    }

    pub fn read_file(&self, path: &Path) -> Result<PathBuf, String> {
        let resolved = path.canonicalize().map_err(|e| format!("cannot open requested file: {e}"))?;
        if !resolved.starts_with(&self.read) || !resolved.is_file() {
            return Err("file is outside the approved read root or is not a file".into());
        }
        Ok(resolved)
    }

    pub fn new_output(&self, path: &Path) -> Result<PathBuf, String> {
        let name = path.file_name().ok_or("output needs a file or folder name")?;
        if name.is_empty() || path.symlink_metadata().is_ok() {
            return Err("output already exists or has no name".into());
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("output needs an absolute parent directory")?;
        let parent = parent.canonicalize().map_err(|e| format!("cannot resolve output parent: {e}"))?;
        if !parent.starts_with(&self.write) {
            return Err("output is outside the approved write root".into());
        }
        Ok(parent.join(name))
    }
}

pub struct Options {
    pub port: u16,
    pub token: String,
    pub roots: Roots,
}

pub fn options_from_args() -> Result<Option<Options>, String> {
    let mut port = None;
    let mut read_root = None;
    let mut write_root = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--control") if port.is_none() => {
                let value = args.next().ok_or("--control needs a TCP port")?;
                let value = value.to_str().ok_or("control port must be UTF-8 digits")?;
                let parsed: u16 = value.parse().map_err(|_| "control port must be 1..65535")?;
                if parsed == 0 {
                    return Err("control port must be 1..65535".into());
                }
                port = Some(parsed);
            }
            Some("--control-read-root") if read_root.is_none() => read_root = Some(PathBuf::from(args.next().ok_or("missing control read root")?)),
            Some("--control-write-root") if write_root.is_none() => write_root = Some(PathBuf::from(args.next().ok_or("missing control write root")?)),
            _ => return Err("unsupported or repeated AnimateStrike argument".into()),
        }
    }
    let Some(port) = port else {
        if read_root.is_some() || write_root.is_some() {
            return Err("control roots require --control".into());
        }
        return Ok(None);
    };
    let token = std::env::var("ANIMATESTRIKE_CONTROL_TOKEN").map_err(|_| "control requires ANIMATESTRIKE_CONTROL_TOKEN".to_string())?;
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("control token must be 64 hexadecimal characters".into());
    }
    let read_root = read_root.ok_or("control requires --control-read-root")?;
    let write_root = write_root.ok_or("control requires --control-write-root")?;
    Ok(Some(Options { port, token, roots: Roots::new(&read_root, &write_root)? }))
}

pub fn start(port: u16, token: String, ctx: egui::Context) -> std::io::Result<Receiver<Request>> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let (tx, rx) = channel();
    std::thread::Builder::new().name("animatestrike-control".into()).spawn(move || accept_loop(listener, Arc::<str>::from(token), tx, ctx))?;
    Ok(rx)
}

fn accept_loop(listener: TcpListener, token: Arc<str>, tx: Sender<Request>, ctx: egui::Context) {
    let active = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        accept_connection(stream, Arc::clone(&token), tx.clone(), ctx.clone(), Arc::clone(&active), DEADLINE);
    }
}

fn accept_connection(mut stream: TcpStream, token: Arc<str>, tx: Sender<Request>, ctx: egui::Context, active: Arc<AtomicUsize>, lifetime: Duration) {
    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
        active.fetch_sub(1, Ordering::SeqCst);
        let _ = stream.set_write_timeout(Some(lifetime));
        let _ = writeln!(stream, "{}", json!({"id": null, "ok": false, "error": "connection limit reached"}));
        return;
    }
    let active_for_worker = Arc::clone(&active);
    let connection_deadline = Instant::now() + lifetime;
    if std::thread::Builder::new()
        .name("animatestrike-client".into())
        .spawn(move || {
            serve(stream, &token, &tx, &ctx, connection_deadline);
            active_for_worker.fetch_sub(1, Ordering::SeqCst);
        })
        .is_err()
    {
        active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct DeadlineRead {
    stream: TcpStream,
    until: Instant,
}

impl Read for DeadlineRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "control connection deadline exceeded"));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buffer)
    }
}

struct DeadlineWrite {
    stream: TcpStream,
    until: Instant,
}

impl Write for DeadlineWrite {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "control connection deadline exceeded"));
        }
        self.stream.set_write_timeout(Some(remaining))?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        if Instant::now() >= self.until {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "control connection deadline exceeded"));
        }
        self.stream.flush()
    }
}

fn serve(stream: TcpStream, token: &str, tx: &Sender<Request>, ctx: &egui::Context, connection_deadline: Instant) {
    if Instant::now() >= connection_deadline {
        return;
    }
    let Ok(read) = stream.try_clone() else { return };
    let write = DeadlineWrite { stream, until: connection_deadline };
    serve_lines(DeadlineRead { stream: read, until: connection_deadline }, write, token, Some(connection_deadline), |method, params| {
        let (reply, rx) = channel();
        let deadline = (Instant::now() + DEADLINE).min(connection_deadline);
        tx.send(Request { method, params, reply, deadline }).ok()?;
        ctx.request_repaint();
        Some(
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| json!({"ok": false, "error": "control timeout; operation may have completed, inspect state before retrying"})),
        )
    });
}

fn token_matches(given: &str, expected: &str) -> bool {
    let a = given.as_bytes();
    let b = expected.as_bytes();
    let mut difference = a.len() ^ b.len();
    for (index, byte) in b.iter().enumerate() {
        difference |= usize::from(a.get(index).copied().unwrap_or_default() ^ *byte);
    }
    difference == 0
}

fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Result<bool, String> {
    buf.clear();
    let count = reader.take((MAX_LINE + 1) as u64).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if count == 0 {
        return Ok(false);
    }
    if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
        return Err("control line too long".into());
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(true)
}

fn serve_lines<R: Read, W: Write>(read: R, mut out: W, token: &str, until: Option<Instant>, mut handle: impl FnMut(String, Value) -> Option<Value>) {
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        if until.is_some_and(|deadline| Instant::now() >= deadline) {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "control connection deadline exceeded"}));
            return;
        }
        let result = read_line(&mut reader, &mut line);
        let Ok(has_line) = result else {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "invalid or oversized control line"}));
            return;
        };
        if !has_line {
            return;
        }
        if until.is_some_and(|deadline| Instant::now() >= deadline) {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "control connection deadline exceeded"}));
            return;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "invalid UTF-8 control line; closing connection"}));
            return;
        };
        if text.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "not a JSON control request; closing connection"}));
            return;
        };
        let Some(object) = message.as_object() else {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "control request must be an object; closing connection"}));
            return;
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            let _ = writeln!(out, "{}", json!({"id": null, "ok": false, "error": "control request needs a method; closing connection"}));
            return;
        };
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let auth = object.get("auth").and_then(Value::as_str).unwrap_or_default();
        if !token_matches(auth, token) {
            let _ = writeln!(out, "{}", json!({"id": id, "ok": false, "error": "unauthorized control request"}));
            return;
        }
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(mut reply) = handle(method.to_string(), params) else { return };
        if let Some(object) = reply.as_object_mut() {
            object.insert("id".into(), id.clone());
        }
        let Ok(encoded) = serde_json::to_vec(&reply) else {
            return;
        };
        if encoded.len() > MAX_REPLY {
            let _ = writeln!(out, "{}", json!({"id": id, "ok": false, "error": "control reply exceeded limit; operation may have completed"}));
            return;
        }
        if out.write_all(&encoded).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn wrong_token_and_http_never_dispatch() {
        for request in [b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(), b"{\"id\":1,\"method\":\"edit.undo\",\"auth\":\"wrong\"}\n".as_slice()] {
            let mut called = false;
            let mut output = Vec::new();
            serve_lines(request, &mut output, TOKEN, None, |_, _| {
                called = true;
                Some(json!({"ok": true}))
            });
            assert!(!called);
        }
    }

    #[test]
    fn authenticated_lines_dispatch_in_order_without_echoing_secret() {
        let input = format!(
            "{{\"id\":1,\"method\":\"state\",\"auth\":\"{TOKEN}\"}}\n{{\"id\":2,\"method\":\"frame.set\",\"auth\":\"{TOKEN}\",\"params\":{{\"frame\":7}}}}\n"
        );
        let mut output = Vec::new();
        let mut calls = Vec::new();
        serve_lines(input.as_bytes(), &mut output, TOKEN, None, |method, params| {
            calls.push((method, params));
            Some(json!({"ok": true, "result": "handled"}))
        });
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "state");
        assert_eq!(calls[1].1["frame"], 7);
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains(TOKEN));
        let replies: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[1]["id"], 2);
    }

    #[test]
    fn roots_reject_escape_and_existing_outputs() {
        let workspace = std::env::temp_dir().join(format!("animatestrike-roots-{}", std::process::id()));
        let read = workspace.join("read");
        let write = workspace.join("write");
        std::fs::create_dir_all(&read).unwrap();
        std::fs::create_dir_all(&write).unwrap();
        std::fs::write(read.join("project.animatestrike"), b"fixture").unwrap();
        std::fs::write(write.join("existing.png"), b"fixture").unwrap();
        std::fs::create_dir(read.join("nested")).unwrap();
        std::fs::create_dir(write.join("nested")).unwrap();
        let roots = Roots::new(&read, &write).unwrap();
        assert!(roots.read_file(&read.join("project.animatestrike")).is_ok());
        assert!(roots.read_file(&read.join("nested/../project.animatestrike")).is_ok());
        assert!(roots.read_file(&write.join("existing.png")).is_err());
        assert!(roots.new_output(&write.join("new.png")).is_ok());
        assert!(roots.new_output(&write.join("existing.png")).is_err());
        assert!(roots.new_output(&write.join("nested/../existing.png")).is_err());
        assert!(roots.new_output(&write.join("nested/../../outside.png")).is_err());
        assert!(roots.new_output(&workspace.join("outside.png")).is_err());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn directory_symlink_alias_cannot_escape_roots_when_supported() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let workspace = std::env::temp_dir().join(format!("animatestrike-alias-{}-{nonce}", std::process::id()));
        let read = workspace.join("read");
        let write = workspace.join("write");
        let outside = workspace.join("outside");
        std::fs::create_dir_all(&read).unwrap();
        std::fs::create_dir_all(&write).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("private.animatestrike"), b"fixture").unwrap();
        let read_alias = read.join("alias");
        let write_alias = write.join("alias");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, &read_alias).and_then(|_| std::os::windows::fs::symlink_dir(&outside, &write_alias));
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &read_alias).and_then(|_| std::os::unix::fs::symlink(&outside, &write_alias));
        if linked.is_ok() {
            let roots = Roots::new(&read, &write).unwrap();
            assert!(roots.read_file(&read_alias.join("private.animatestrike")).is_err());
            assert!(roots.new_output(&write_alias.join("escaped.png")).is_err());
        }
        for alias in [&read_alias, &write_alias] {
            if alias.symlink_metadata().is_ok() {
                #[cfg(windows)]
                std::fs::remove_dir(alias).unwrap();
                #[cfg(unix)]
                std::fs::remove_file(alias).unwrap();
            }
        }
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn trickling_peers_release_all_four_slots_at_absolute_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let server_active = Arc::clone(&active);
        let (tx, _rx) = channel::<Request>();
        let server = std::thread::spawn(move || {
            for _ in 0..6 {
                let (stream, _) = listener.accept().unwrap();
                accept_connection(stream, Arc::from(TOKEN), tx.clone(), egui::Context::default(), Arc::clone(&server_active), Duration::from_secs(2));
            }
        });
        let mut slow = Vec::new();
        let mut writers = Vec::new();
        for _ in 0..4 {
            let stream = TcpStream::connect(address).unwrap();
            let mut writer = stream.try_clone().unwrap();
            writers.push(std::thread::spawn(move || {
                for _ in 0..70 {
                    if writer.write_all(b"x").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
            }));
            slow.push(stream);
        }
        let admitted_by = Instant::now() + Duration::from_secs(2);
        while active.load(Ordering::SeqCst) < 4 && Instant::now() < admitted_by {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(active.load(Ordering::SeqCst), 4);
        let mut blocked = TcpStream::connect(address).unwrap();
        blocked.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut refused = String::new();
        BufReader::new(&mut blocked).read_line(&mut refused).unwrap();
        assert!(refused.contains("connection limit reached"));
        let released_by = Instant::now() + Duration::from_secs(3);
        while active.load(Ordering::SeqCst) != 0 && Instant::now() < released_by {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(active.load(Ordering::SeqCst), 0, "trickled bytes must not extend the connection lifetime");
        let mut resumed = TcpStream::connect(address).unwrap();
        resumed.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        writeln!(resumed, "{}", json!({"id": 6, "method": "state", "auth": "wrong"})).unwrap();
        let mut reply = String::new();
        BufReader::new(&mut resumed).read_line(&mut reply).unwrap();
        assert!(reply.contains("unauthorized control request"), "new clients must be admitted after slow peers expire: {reply}");
        for writer in writers {
            writer.join().unwrap();
        }
        drop(slow);
        server.join().unwrap();
    }
}
