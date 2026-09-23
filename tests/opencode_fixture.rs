//! A synthetic loopback server imitating the OpenCode routes htalk uses; no real OpenCode runs.
//! Included by the other `opencode_*` test files through `#[path]`.
#![allow(dead_code)]
use base64::Engine;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Reply {
    Json(u16, Value),
    Empty(u16),
    Raw(Vec<u8>),
    Close,
    Sleep(Duration),
}
pub type Hook = Box<dyn Fn(&Request) -> Option<Reply> + Send>;

pub struct State {
    pub sessions: Vec<Value>,
    pub status: Value,
    pub prompt_status: u16,
    pub password: Option<String>,
    pub requests: Vec<Request>,
    pub hook: Option<Hook>,
}

pub struct Fake {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}
impl Fake {
    pub fn start(sessions: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("address").port();
        let state = Arc::new(Mutex::new(State {
            sessions,
            status: json!({}),
            prompt_status: 204,
            password: None,
            requests: Vec::new(),
            hook: None,
        }));
        let shared = state.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            state,
        }
    }
    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn posts(&self) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }
    pub fn hook(&self, hook: impl Fn(&Request) -> Option<Reply> + Send + 'static) {
        self.state.lock().unwrap().hook = Some(Box::new(hook));
    }
    pub fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }
}

pub fn session(id: &str, directory: impl Into<Value>) -> Value {
    json!({"id": id, "slug": "synthetic", "projectID": "prj_synthetic", "directory": directory.into(),
        "title": "synthetic", "version": "1.18.30", "time": {"created": 1000000, "updated": 1788990000000i64}})
}

/// A loopback port with nothing listening.
pub fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener.local_addr().expect("address").port()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1
            }
            b'%' if i + 2 < bytes.len() && u8::from_str_radix(&text[i + 1..i + 3], 16).is_ok() => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 3
            }
            b => {
                out.push(b);
                i += 1
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        data.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&data).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: Vec<(String, String)> = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.split_once(':')
                .map(|(n, v)| (n.to_owned(), v.trim().to_owned()))
        })
        .collect();
    let length = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).ok()?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect();
    Some(Request {
        method,
        path: path.to_owned(),
        query,
        headers,
        body,
    })
}

fn serve(mut stream: TcpStream, state: &Mutex<State>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    let reply = {
        let mut state = state.lock().unwrap();
        state.requests.push(request.clone());
        route(&state, &request)
    };
    let bytes = match reply {
        Reply::Close => return,
        Reply::Sleep(d) => {
            thread::sleep(d);
            return;
        }
        Reply::Raw(bytes) => bytes,
        Reply::Empty(status) => {
            format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n").into_bytes()
        }
        Reply::Json(status, value) => {
            let body = serde_json::to_vec(&value).unwrap();
            let mut out = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
            out.extend(body);
            out
        }
    };
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

fn route(state: &State, request: &Request) -> Reply {
    if let Some(reply) = state.hook.as_ref().and_then(|h| h(request)) {
        return reply;
    }
    if let Some(password) = &state.password {
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("opencode:{password}"))
        );
        if request.header("authorization") != Some(expected.as_str()) {
            return Reply::Json(
                401,
                json!({"name": "UnauthorizedError", "data": {"message": "Unauthorized"}}),
            );
        }
    }
    let parts: Vec<&str> = request.path.split('/').collect();
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/global/health") => {
            return Reply::Json(200, json!({"healthy": true, "version": "1.18.30"}));
        }
        ("GET", "/session") => return Reply::Json(200, Value::Array(state.sessions.clone())),
        ("GET", "/session/status") => return Reply::Json(200, state.status.clone()),
        _ => (),
    }
    let id = parts.get(2).copied().unwrap_or("");
    let Some(session) = state.sessions.iter().find(|s| s["id"] == id) else {
        return Reply::Json(
            404,
            json!({"name": "NotFoundError", "data": {"message": format!("Session not found: {id}")}}),
        );
    };
    match (request.method.as_str(), &parts[3..]) {
        ("GET", []) => Reply::Json(200, session.clone()),
        ("POST", ["prompt_async"]) => Reply::Empty(state.prompt_status),
        _ => Reply::Json(
            404,
            json!({"name": "NotFoundError", "data": {"message": "no route"}}),
        ),
    }
}

/// A private temporary directory removed on drop; tempfile is not in the offline cache.
pub struct TempDir(std::path::PathBuf);
impl TempDir {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub fn tempdir() -> TempDir {
    use std::{
        os::unix::fs::DirBuilderExt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "htalk-opencode-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .expect("create temporary directory");
    TempDir(path)
}
