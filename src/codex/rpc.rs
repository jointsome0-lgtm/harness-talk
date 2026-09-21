//! Bounded JSON RPC to a Codex app-server over its Unix WebSocket or a temporary stdio process.
use super::{connect_unix, io_failure, owned_socket, python_dumps, socket_failure};
use crate::error::Failure;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio as Pipe},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, handshake::HandshakeError, protocol::WebSocketConfig};

const FRAME_LIMIT: usize = 4 * 1024 * 1024;
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// An initialized RPC connection. Dropping it closes the WebSocket, or ends the stdio process
/// by closing stdin, then terminating, then killing, each bounded by one second.
pub struct Rpc {
    connection: Connection,
    counter: i64,
}

enum Connection {
    Socket(Box<WebSocket<UnixStream>>),
    Stdio(StdioProcess),
}

struct StdioProcess {
    child: Child,
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
        let path = owned_socket(path)?;
        let deadline = Instant::now() + OPEN_TIMEOUT;
        let stream = connect_unix(&path, OPEN_TIMEOUT).map_err(io_failure)?;
        let remaining = remaining(deadline).ok_or(Failure::Class("TimeoutError"))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(io_failure)?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(io_failure)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(FRAME_LIMIT))
            .max_frame_size(Some(FRAME_LIMIT));
        let (socket, _) =
            tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))
                .map_err(|error| match error {
                    HandshakeError::Interrupted(_) => Failure::Class("TimeoutError"),
                    HandshakeError::Failure(tungstenite::Error::Io(error)) => socket_failure(error),
                    HandshakeError::Failure(_) => websocket_failure(),
                })?;
        socket
            .get_ref()
            .set_write_timeout(Some(CALL_TIMEOUT))
            .map_err(io_failure)?;
        let mut rpc = Self {
            connection: Connection::Socket(Box::new(socket)),
            counter: 0,
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
            .spawn()
            .map_err(io_failure)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Class("OSError"));
        };
        let mut rpc = Self {
            connection: Connection::Stdio(StdioProcess {
                child,
                stdin: Some(stdin),
                stdout,
                buffer: Vec::new(),
            }),
            counter: 0,
        };
        rpc.initialize()?;
        Ok(rpc)
    }

    /// Send one request and return its result. Frames for other IDs are skipped. A rejection
    /// keeps only an integer RPC code, never the server's message.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        self.counter += 1;
        self.write(&json!({"id": self.counter, "method": method, "params": params}))?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let remaining =
                remaining(deadline).ok_or_else(|| Failure::coded("codex_rpc_timeout"))?;
            let frame = self.recv(remaining)?;
            let frame: Value =
                serde_json::from_str(&frame).map_err(|_| Failure::Class("JSONDecodeError"))?;
            let frame = frame.as_object().ok_or(Failure::Class("AttributeError"))?;
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
            return frame
                .get("result")
                .cloned()
                .ok_or(Failure::Class("KeyError"));
        }
    }

    fn initialize(&mut self) -> Result<(), Failure> {
        self.call(
            "initialize",
            json!({"clientInfo": {"name": "harness-talk", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}}),
        )?;
        self.write(&json!({"method": "initialized"}))
    }

    fn write(&mut self, frame: &Value) -> Result<(), Failure> {
        let text = python_dumps(frame);
        match &mut self.connection {
            Connection::Socket(socket) => socket
                .send(Message::text(text))
                .map_err(|_| websocket_failure()),
            Connection::Stdio(process) => {
                let stdin = process.stdin.as_mut().ok_or(Failure::Class("ValueError"))?;
                stdin
                    .write_all(format!("{text}\n").as_bytes())
                    .and_then(|_| stdin.flush())
                    .map_err(io_failure)
            }
        }
    }

    fn recv(&mut self, timeout: Duration) -> Result<String, Failure> {
        match &mut self.connection {
            Connection::Socket(socket) => {
                let deadline = Instant::now() + timeout;
                loop {
                    let remaining = remaining(deadline).ok_or(Failure::Class("TimeoutError"))?;
                    socket
                        .get_ref()
                        .set_read_timeout(Some(remaining))
                        .map_err(io_failure)?;
                    match socket.read() {
                        Ok(Message::Text(text)) => return Ok(text.as_str().to_owned()),
                        Ok(Message::Binary(bytes)) => {
                            return String::from_utf8(bytes.to_vec())
                                .map_err(|_| Failure::Class("UnicodeDecodeError"));
                        }
                        Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
                        Ok(Message::Close(_)) => return Err(websocket_failure()),
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            return Err(Failure::Class("TimeoutError"));
                        }
                        Err(_) => return Err(websocket_failure()),
                    }
                }
            }
            Connection::Stdio(process) => process.recv(timeout),
        }
    }
}

impl StdioProcess {
    /// Bounded newline framing for the local app-server process.
    fn recv(&mut self, timeout: Duration) -> Result<String, Failure> {
        let deadline = Instant::now() + timeout;
        while !self.buffer.contains(&b'\n') {
            if self.buffer.len() >= FRAME_LIMIT {
                return Err(Failure::coded("codex_rpc_frame_too_large"));
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            let mut poll = libc::pollfd {
                fd: self.stdout.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let milliseconds = wait.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32;
            // SAFETY: polls one descriptor owned by this process handle.
            match unsafe { libc::poll(&mut poll, 1, milliseconds) } {
                0 => return Err(Failure::coded("codex_rpc_timeout")),
                n if n < 0 => {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(io_failure(error));
                }
                _ => (),
            }
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
            return Err(Failure::Class("OSError"));
        };
        let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
        line.pop();
        if line.len() > FRAME_LIMIT {
            return Err(Failure::coded("codex_rpc_frame_too_large"));
        }
        String::from_utf8(line).map_err(|_| Failure::Class("UnicodeDecodeError"))
    }

    fn exited_within(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        match &mut self.connection {
            Connection::Socket(socket) => {
                let deadline = Instant::now() + CLOSE_TIMEOUT;
                let _ = socket.get_ref().set_write_timeout(Some(CLOSE_TIMEOUT));
                if socket.close(None).is_err() {
                    return;
                }
                while let Some(remaining) = remaining(deadline) {
                    if socket.get_ref().set_read_timeout(Some(remaining)).is_err()
                        || socket.read().is_err()
                    {
                        break;
                    }
                }
            }
            Connection::Stdio(process) => {
                drop(process.stdin.take());
                if process.exited_within(CLOSE_TIMEOUT) {
                    return;
                }
                // SAFETY: signals only the child this handle spawned and has not yet reaped.
                unsafe {
                    libc::kill(process.child.id() as libc::pid_t, libc::SIGTERM);
                }
                if process.exited_within(CLOSE_TIMEOUT) {
                    return;
                }
                let _ = process.child.kill();
                process.exited_within(CLOSE_TIMEOUT);
            }
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
