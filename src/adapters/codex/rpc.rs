//! Bounded JSON RPC to a Codex app-server over its Unix WebSocket or a temporary stdio process.
use crate::{
    compat::{self, io_failure, owned_socket, python_dumps, socket_failure},
    error::Failure,
    os::{self, Grouped, OwnedGroup, connect_unix},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio as Pipe},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, handshake::HandshakeError, protocol::WebSocketConfig};

const FRAME_LIMIT: usize = 4 * 1024 * 1024;
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Owner-selected socket instance. Replacing the listener requires an explicit rebind.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct BoundSocket {
    pub path: PathBuf,
    device: u64,
    inode: u64,
    changed_seconds: i64,
    changed_nanos: i64,
}

impl BoundSocket {
    pub(crate) fn capture(path: PathBuf) -> Result<Self, Failure> {
        let path = owned_socket(&path)?;
        let meta = std::fs::metadata(&path).map_err(io_failure)?;
        let ((device, inode), (changed_seconds, changed_nanos)) =
            (os::file_id(&meta), os::changed(&meta));
        Ok(Self {
            path,
            device,
            inode,
            changed_seconds,
            changed_nanos,
        })
    }

    pub(crate) fn check(&self) -> Result<(), Failure> {
        if Self::capture(self.path.clone())? != *self {
            return Err(Failure::coded("codex_server_socket_changed"));
        }
        Ok(())
    }
}

/// An initialized RPC connection. Use `close` to check local transport cleanup.
/// Drop attempts bounded cleanup but cannot report whether it completed.
pub struct Rpc {
    connection: Connection,
    counter: i64,
    closing: bool,
}

enum Connection {
    Closed,
    Socket(Box<WebSocket<DeadlineStream>>),
    Stdio(StdioProcess),
}

struct StdioProcess {
    child: Child,
    group: OwnedGroup,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    buffer: Vec<u8>,
}

fn websocket_failure() -> Failure {
    Failure::coded("codex_websocket_failure")
}

impl Rpc {
    /// Connect to an owned Unix socket without compression and initialize the protocol.
    pub fn connect_unix(path: &Path) -> Result<Self, Failure> {
        Self::connect_unix_checked(path, None)
    }

    pub(crate) fn connect_bound(socket: &BoundSocket) -> Result<Self, Failure> {
        Self::connect_unix_checked(&socket.path, Some(socket))
    }

    fn connect_unix_checked(path: &Path, bound: Option<&BoundSocket>) -> Result<Self, Failure> {
        let path = owned_socket(path)?;
        if let Some(bound) = bound {
            bound.check()?;
        }
        let deadline = Instant::now() + OPEN_TIMEOUT;
        let stream = connect_unix(&path, OPEN_TIMEOUT).map_err(io_failure)?;
        if let Some(bound) = bound {
            bound.check()?;
        }
        let stream = DeadlineStream { stream, deadline };
        let config = WebSocketConfig::default()
            .max_message_size(Some(FRAME_LIMIT))
            .max_frame_size(Some(FRAME_LIMIT));
        let (socket, _) =
            tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))
                .map_err(|error| match error {
                    HandshakeError::Interrupted(_) => compat::TIMEOUT_ERROR,
                    HandshakeError::Failure(tungstenite::Error::Io(error)) => socket_failure(error),
                    HandshakeError::Failure(_) => websocket_failure(),
                })?;
        let mut rpc = Self {
            connection: Connection::Socket(Box::new(socket)),
            counter: 0,
            closing: false,
        };
        rpc.initialize()?;
        Ok(rpc)
    }

    /// Start `codex app-server --stdio` with the user's normal configuration and initialize it.
    /// No thread is started or resumed.
    pub fn spawn_stdio() -> Result<Self, Failure> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .stdin(Pipe::piped())
            .stdout(Pipe::piped())
            .stderr(Pipe::null())
            .own_group()
            .spawn()
            .map_err(io_failure)?;
        let mut group = match OwnedGroup::new(child.id()) {
            Ok(group) => group,
            Err(_) => {
                // Ownership setup failed before any RPC write. Signal only the Child
                // we spawned, and bound the direct-child reap attempt.
                let _ = child.kill();
                let deadline = Instant::now() + CLOSE_TIMEOUT;
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                return Err(Failure::coded("codex_stdio_cleanup_ownership_unavailable"));
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            if group.finish().is_ok() && group.exited().unwrap_or(false) {
                let _ = child.wait();
            } else {
                let _ = group.kill_leader();
            }
            return Err(Failure::coded("codex_stdio_cleanup_unconfirmed"));
        };
        let mut rpc = Self {
            connection: Connection::Stdio(StdioProcess {
                child,
                group,
                stdin: Some(stdin),
                stdout,
                buffer: Vec::new(),
            }),
            counter: 0,
            closing: false,
        };
        if let Connection::Stdio(process) = &rpc.connection {
            os::set_nonblocking(process.stdin.as_ref().unwrap()).map_err(io_failure)?;
        }
        rpc.initialize()?;
        Ok(rpc)
    }

    /// Send one request and return its result. Frames for other IDs are skipped. A rejection
    /// keeps only an integer RPC code, never the server's message. One ten-second
    /// deadline covers request writing and response reading. A failed write may be partial;
    /// callers must preserve submission uncertainty and must not replay it.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        self.call_until(method, params, Instant::now() + CALL_TIMEOUT)
    }

    fn call_until(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, Failure> {
        if self.closing {
            return Err(Failure::coded("codex_rpc_closed"));
        }
        self.counter += 1;
        self.write(
            &json!({"id": self.counter, "method": method, "params": params}),
            deadline,
        )?;
        loop {
            remaining(deadline).ok_or_else(|| Failure::coded("codex_rpc_timeout"))?;
            let frame = self.recv(deadline)?;
            let frame: Value =
                serde_json::from_str(&frame).map_err(|_| compat::JSON_DECODE_ERROR)?;
            let frame = frame.as_object().ok_or(compat::ATTRIBUTE_ERROR)?;
            if !same_id(frame.get("id"), self.counter) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                let code =
                    error
                        .as_object()
                        .and_then(|e| e.get("code"))
                        .and_then(|code| match code {
                            Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
                            _ => None,
                        });
                return Err(Failure::coded(match code {
                    Some(code) => format!("codex_rpc_rejected:{code}"),
                    None => "codex_rpc_rejected".to_string(),
                }));
            }
            return frame.get("result").cloned().ok_or(compat::KEY_ERROR);
        }
    }

    /// Finish local transport cleanup. Every close attempt is terminal for RPC calls,
    /// including failure. A failed stdio close retains its unreaped child and pinned
    /// group for a safe retry; it never changes a prior request's confirmed receipt.
    ///
    /// Stdio cleanup gives EOF one second, then uses a four-second TERM/KILL observation
    /// budget checked between process scans. Scans and scheduling can extend elapsed time.
    /// Reaping follows observed direct-child exit.
    /// Retries omit the EOF grace. `os::OwnedGroup` and what it reads of each process are
    /// required; descendants that leave the private group are outside this contract.
    /// Socket cleanup closes the local stream after at most one second of close I/O;
    /// it does not confirm remote consumption of any request.
    pub fn close(&mut self) -> Result<(), Failure> {
        let first_attempt = !self.closing;
        self.closing = true;
        match &mut self.connection {
            Connection::Closed => return Ok(()),
            Connection::Socket(socket) => {
                let deadline = Instant::now() + CLOSE_TIMEOUT;
                socket.get_mut().deadline = deadline;
                if socket.close(None).is_ok() {
                    while remaining(deadline).is_some() && socket.read().is_ok() {}
                }
                socket
                    .get_mut()
                    .stream
                    .shutdown(std::net::Shutdown::Both)
                    .map_err(io_failure)?;
            }
            Connection::Stdio(process) => process.close(first_attempt)?,
        }
        self.connection = Connection::Closed;
        Ok(())
    }

    fn initialize(&mut self) -> Result<(), Failure> {
        self.call(
            "initialize",
            json!({"clientInfo": {"name": "harness-talk", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}}),
        )?;
        self.write(
            &json!({"method": "initialized"}),
            Instant::now() + CALL_TIMEOUT,
        )
    }

    fn write(&mut self, frame: &Value, deadline: Instant) -> Result<(), Failure> {
        let text = python_dumps(frame);
        match &mut self.connection {
            Connection::Closed => Err(Failure::coded("codex_rpc_closed")),
            Connection::Socket(socket) => {
                socket.get_mut().deadline = deadline;
                socket
                    .send(Message::text(text))
                    .map_err(|error| match error {
                        tungstenite::Error::Io(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            compat::TIMEOUT_ERROR
                        }
                        _ => websocket_failure(),
                    })
            }
            Connection::Stdio(process) => process.write(format!("{text}\n").as_bytes(), deadline),
        }
    }

    fn recv(&mut self, deadline: Instant) -> Result<String, Failure> {
        match &mut self.connection {
            Connection::Closed => Err(Failure::coded("codex_rpc_closed")),
            Connection::Socket(socket) => {
                socket.get_mut().deadline = deadline;
                loop {
                    remaining(deadline).ok_or(compat::TIMEOUT_ERROR)?;
                    match socket.read() {
                        Ok(Message::Text(text)) => return Ok(text.as_str().to_owned()),
                        Ok(Message::Binary(bytes)) => {
                            return String::from_utf8(bytes.to_vec())
                                .map_err(|_| compat::UNICODE_DECODE_ERROR);
                        }
                        Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
                        Ok(Message::Close(_)) => return Err(websocket_failure()),
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            return Err(compat::TIMEOUT_ERROR);
                        }
                        Err(_) => return Err(websocket_failure()),
                    }
                }
            }
            Connection::Stdio(process) => process.recv(deadline),
        }
    }
}

// Tungstenite can perform several reads/writes inside one handshake or frame operation.
// Recompute the remaining time at the underlying I/O boundary, not just around socket.read().
struct DeadlineStream {
    stream: os::Socket,
    deadline: Instant,
}

fn deadline_timeout() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, "RPC deadline expired")
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.stream
                .set_read_timeout(Some(remaining(self.deadline).ok_or_else(deadline_timeout)?))?;
            match self.stream.read(buf) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        loop {
            self.stream
                .set_write_timeout(Some(remaining(self.deadline).ok_or_else(deadline_timeout)?))?;
            match self.stream.write(buf) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        remaining(self.deadline).ok_or_else(deadline_timeout)?;
        self.stream.flush()
    }
}

fn stdio_failure(error: std::io::Error) -> Failure {
    if error.kind() == std::io::ErrorKind::TimedOut {
        Failure::coded("codex_rpc_timeout")
    } else {
        io_failure(error)
    }
}

fn poll_until(io: &impl os::Descriptor, writable: bool, deadline: Instant) -> std::io::Result<()> {
    loop {
        let wait = remaining(deadline).ok_or_else(deadline_timeout)?;
        match os::ready(io, writable, wait) {
            Ok(false) => return Err(deadline_timeout()),
            Err(error) if error.kind() != std::io::ErrorKind::Interrupted => return Err(error),
            Err(_) => (),
            Ok(true) => {
                remaining(deadline).ok_or_else(deadline_timeout)?;
                return Ok(());
            }
        }
    }
}

impl StdioProcess {
    fn write(&mut self, mut bytes: &[u8], deadline: Instant) -> Result<(), Failure> {
        let stdin = self.stdin.as_mut().ok_or(compat::VALUE_ERROR)?;
        while !bytes.is_empty() {
            remaining(deadline).ok_or_else(|| Failure::coded("codex_rpc_timeout"))?;
            match stdin.write(bytes) {
                Ok(0) => return Err(compat::OS_ERROR),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    poll_until(stdin, true, deadline).map_err(stdio_failure)?;
                }
                Err(error) => return Err(io_failure(error)),
            }
        }
        Ok(())
    }

    /// Bounded newline framing for the local app-server process.
    fn recv(&mut self, deadline: Instant) -> Result<String, Failure> {
        remaining(deadline).ok_or_else(|| Failure::coded("codex_rpc_timeout"))?;
        while !self.buffer.contains(&b'\n') {
            if self.buffer.len() >= FRAME_LIMIT {
                return Err(Failure::coded("codex_rpc_frame_too_large"));
            }
            poll_until(&self.stdout, false, deadline).map_err(stdio_failure)?;
            let mut chunk = vec![0; 65536];
            let count = match self.stdout.read(&mut chunk) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_failure(error)),
            };
            if count == 0 {
                return Err(Failure::coded("codex_rpc_closed"));
            }
            self.buffer.extend_from_slice(&chunk[..count]);
        }
        let Some(end) = self.buffer.iter().position(|b| *b == b'\n') else {
            return Err(compat::OS_ERROR);
        };
        let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
        line.pop();
        if line.len() > FRAME_LIMIT {
            return Err(Failure::coded("codex_rpc_frame_too_large"));
        }
        String::from_utf8(line).map_err(|_| compat::UNICODE_DECODE_ERROR)
    }

    fn close(&mut self, grace: bool) -> Result<(), Failure> {
        drop(self.stdin.take());
        let cleanup = (|| -> std::io::Result<()> {
            if grace {
                let deadline = Instant::now() + CLOSE_TIMEOUT;
                while !self.group.exited()? && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            // Always clean the group, even after prompt wrapper exit. Keep the wrapper
            // unreaped until the group is stopped so its PID reserves the group number.
            self.group.finish()?;
            if !self.group.exited()? {
                return Err(std::io::Error::other("Direct child exit not confirmed"));
            }
            self.child.wait()?;
            Ok(())
        })();
        cleanup.map_err(|_| Failure::coded("codex_stdio_cleanup_unconfirmed"))
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        if self.close().is_err()
            && let Connection::Stdio(process) = &self.connection
        {
            // A stable leader handle is safe even after group cleanup fails. Do
            // not reap its anchor or claim descendant completion on this path.
            let _ = process.group.kill_leader();
        }
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then(|| left.max(Duration::from_millis(1)))
}

/// Python's `frame.get("id") == counter`, where `1.0` and `True` also equal 1.
fn same_id(id: Option<&Value>, counter: i64) -> bool {
    match id {
        Some(Value::Number(n)) => {
            n.as_i64() == Some(counter)
                || (!n.is_i64() && !n.is_u64() && n.as_f64() == Some(counter as f64))
        }
        Some(Value::Bool(true)) => counter == 1,
        _ => false,
    }
}
