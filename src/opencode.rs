//! OpenCode sessions through the official local HTTP server API.
//!
//! Discovery and checks use GET only and never create a session. Delivery is one
//! POST /session/{id}/prompt_async, which intentionally starts a turn in the
//! existing session. No credential is stored.
//!
//! The HTTP/1.1 exchange follows Python's `http.client`, which 0.4.0 used: one
//! connection per request, no redirects, no proxy, 5 s per socket operation and
//! at most 4 MiB of response body. `opencode_unreachable` is reported only when
//! the TCP connection (and TLS handshake) failed before any request byte was
//! written; every later failure is uncertain.
use crate::model::NativePeer as Peer;
use crate::{
    error::{Error, Failure},
    model::*,
    os, validate,
};
use base64::Engine;
use rusqlite::{OpenFlags, types::ValueRef};
use serde_json::{Map, Value, json};
use std::{
    env, fs,
    io::{self, Read, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const DEFAULT_URL: &str = "http://127.0.0.1:4096";
const TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_RESPONSE: usize = 4 * 1024 * 1024;
pub const SAVED_LIMIT: usize = 50;
const STATUSES: [&str; 3] = ["idle", "busy", "retry"];
const MAX_LINE: usize = 65536;
const MAX_HEADERS: usize = 100;

/// A request failure; `After` means the request may have reached the server.
enum Req {
    Before(Failure),
    After(Failure),
}
impl From<Req> for Failure {
    fn from(r: Req) -> Self {
        match r {
            Req::Before(f) | Req::After(f) => f,
        }
    }
}
fn coded(code: &str) -> Failure {
    Failure::coded(code)
}
fn uncertain(class: &str) -> Req {
    Req::After(Failure::coded(format!("opencode_{class}")))
}
fn from_error(e: Error) -> Failure {
    match e {
        Error::Code(c) => Failure::Coded(c),
        Error::Value(_) => Failure::Class("ValueError"),
        Error::Io(e) => e.into(),
        Error::Db(e) => e.into(),
        Error::Interrupted => Failure::Class("KeyboardInterrupt"),
    }
}
fn session_id(value: &str) -> Result<String, Failure> {
    validate::opencode_session_id(value).map_err(from_error)
}

/// Basic auth from the same environment the server reads; never persisted.
fn credentials() -> Result<Option<String>, Failure> {
    let password = env::var_os("OPENCODE_SERVER_PASSWORD").filter(|p| !p.is_empty());
    let Some(password) = password else {
        return Ok(None);
    };
    let user = env::var_os("OPENCODE_SERVER_USERNAME").filter(|u| !u.is_empty());
    let user = user.as_deref().map_or(Some("opencode"), |u| u.to_str());
    let (Some(user), Some(password)) = (user, password.to_str()) else {
        return Err(Failure::Class("UnicodeEncodeError"));
    };
    Ok(Some(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    )))
}

/// `urllib.parse.quote(value, safe="")`; with `plus`, `quote_plus`.
fn quote(value: &str, plus: bool) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                (b as char).to_string()
            }
            b' ' if plus => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `json.dumps` of a string with its default `ensure_ascii`.
fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("\\u{unit:04x}"))
                }
            }
        }
    }
    out.push('"');
    out
}

/// Python `int(text, radix)` for ASCII text: whitespace, sign, `0x` and digit underscores.
fn py_int(text: &[u8], radix: u32) -> Option<i128> {
    let text = text.trim_ascii();
    let (negative, mut digits) = match text.first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut after_prefix = false;
    if radix == 16 && (digits.starts_with(b"0x") || digits.starts_with(b"0X")) {
        digits = &digits[2..];
        after_prefix = true;
    }
    if after_prefix && digits.first() == Some(&b'_') {
        digits = &digits[1..];
    }
    if digits.is_empty()
        || digits.first() == Some(&b'_')
        || digits.last() == Some(&b'_')
        || digits.windows(2).any(|w| w == b"__")
    {
        return None;
    }
    let mut value: i128 = 0;
    for &b in digits.iter().filter(|b| **b != b'_') {
        let digit = (b as char).to_digit(radix)?;
        value = value
            .saturating_mul(radix as i128)
            .saturating_add(digit as i128);
    }
    Some(if negative { -value } else { value })
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `int(updated // 1000) if type(updated) is int else None`.
fn seconds(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(n)) if n.is_i64() => n
            .as_i64()
            .map_or(Value::Null, |v| json!(v.div_euclid(1000))),
        Some(Value::Number(n)) if n.is_u64() => n.as_u64().map_or(Value::Null, |v| json!(v / 1000)),
        _ => Value::Null,
    }
}

enum Conn {
    Plain(TcpStream),
    Tls(Box<ureq::unversioned::transport::TransportAdapter>),
}
impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            // A TLS peer that closes without close_notify ends the body, as Python's ragged-EOF rule.
            Self::Tls(s) => match s.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                other => other,
            },
        }
    }
}
impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

/// Python exception class for a socket or TLS failure after the connection opened.
fn io_class(e: &io::Error) -> &'static str {
    if let Some(inner) = e.get_ref().and_then(|x| x.downcast_ref::<ureq::Error>()) {
        return match inner {
            ureq::Error::Timeout(_) => "TimeoutError",
            ureq::Error::Io(e) => io_class(e),
            _ => "SSLError",
        };
    }
    Failure::from(io::Error::from(e.kind())).class_name()
}

/// Trust roots as Python's default context loads them: SSL_CERT_FILE or the system bundle, plus SSL_CERT_DIR.
fn trust_roots() -> Vec<ureq::tls::Certificate<'static>> {
    const BUNDLES: [&str; 5] = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/pki/tls/cacert.pem",
        "/etc/ssl/cert.pem",
    ];
    let mut files: Vec<PathBuf> = match env::var_os("SSL_CERT_FILE").filter(|f| !f.is_empty()) {
        Some(file) => vec![PathBuf::from(file)],
        None => BUNDLES
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .into_iter()
            .collect(),
    };
    if let Some(dir) = env::var_os("SSL_CERT_DIR").filter(|d| !d.is_empty())
        && let Ok(entries) = fs::read_dir(dir)
    {
        let mut listed: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        listed.sort();
        files.extend(listed);
    }
    let mut roots = Vec::new();
    for file in files {
        let Ok(pem) = fs::read(&file) else { continue };
        for item in ureq::tls::parse_pem(&pem).flatten() {
            if let ureq::tls::PemItem::Certificate(cert) = item {
                roots.push(cert.to_owned());
            }
        }
    }
    roots
}

/// The open TCP stream under TLS; its socket already carries the 5 s timeouts.
#[derive(Debug)]
struct Tcp {
    stream: TcpStream,
    buffers: ureq::unversioned::transport::LazyBuffers,
}
impl ureq::unversioned::transport::Transport for Tcp {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        &mut self.buffers
    }
    fn transmit_output(
        &mut self,
        amount: usize,
        _: ureq::unversioned::transport::NextTimeout,
    ) -> Result<(), ureq::Error> {
        use ureq::unversioned::transport::Buffers;
        Ok(self.stream.write_all(&self.buffers.output()[..amount])?)
    }
    fn await_input(
        &mut self,
        _: ureq::unversioned::transport::NextTimeout,
    ) -> Result<bool, ureq::Error> {
        use ureq::unversioned::transport::Buffers;
        let amount = self.stream.read(self.buffers.input_append_buf())?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }
    fn is_open(&mut self) -> bool {
        true
    }
}

/// TLS handshake on an open TCP stream, completed before any request byte is sent.
fn tls(stream: TcpStream, host: &str, port: u16) -> Result<Conn, ureq::Error> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    use ureq::unversioned::{
        resolver::{DefaultResolver, Resolver},
        transport::{
            ConnectionDetails, Connector, LazyBuffers, NextTimeout, RustlsConnector, Transport,
            TransportAdapter, time,
        },
    };
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let uri: ureq::http::Uri = format!("https://{authority}/")
        .parse()
        .map_err(|_| ureq::Error::ConnectionFailed)?;
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::Specific(Arc::new(trust_roots())))
        .build();
    let config = ureq::config::Config::builder().tls_config(tls).build();
    let resolver = DefaultResolver::default();
    let details = ConnectionDetails {
        uri: &uri,
        addrs: resolver.empty(),
        config: &config,
        request_level: false,
        resolver: &resolver,
        now: time::Instant::now(),
        timeout: NextTimeout {
            after: TIMEOUT.into(),
            reason: ureq::Timeout::Connect,
        },
        current_time: Arc::new(time::Instant::now),
        run_connector: Arc::new(|_: &ConnectionDetails| Err(ureq::Error::ConnectionFailed)),
    };
    let tcp = Tcp {
        stream,
        buffers: LazyBuffers::new(128 * 1024, 128 * 1024),
    };
    let transport = RustlsConnector::default()
        .connect(&details, Some(tcp))?
        .ok_or(ureq::Error::ConnectionFailed)?;
    let mut adapter = TransportAdapter::new(transport.boxed());
    adapter.set_timeout(NextTimeout {
        after: TIMEOUT.into(),
        reason: ureq::Timeout::RecvResponse,
    });
    Ok(Conn::Tls(Box::new(adapter)))
}

/// Buffered reads with `http.client`'s limits and exception classes.
struct Reader {
    conn: Conn,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}
impl Reader {
    fn fill(&mut self) -> Result<bool, &'static str> {
        if self.eof {
            return Ok(false);
        }
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        let mut chunk = [0u8; 16 * 1024];
        let n = loop {
            match self.conn.read(&mut chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_class(&e)),
            }
        };
        if n == 0 {
            self.eof = true;
            return Ok(false);
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(true)
    }
    fn readline(&mut self, limit: usize) -> Result<Vec<u8>, &'static str> {
        loop {
            let pending = &self.buf[self.pos..];
            let end = pending.iter().position(|b| *b == b'\n').map(|i| i + 1);
            if let Some(end) = end.filter(|e| *e <= limit) {
                self.pos += end;
                return Ok(pending[..end].to_vec());
            }
            if pending.len() >= limit {
                self.pos += limit;
                return Ok(pending[..limit].to_vec());
            }
            if !self.fill()? {
                let line = pending_owned(&self.buf, self.pos);
                self.pos = self.buf.len();
                return Ok(line);
            }
        }
    }
    /// Up to `n` bytes, fewer only at EOF.
    fn read(&mut self, n: usize) -> Result<Vec<u8>, &'static str> {
        while self.buf.len() - self.pos < n && self.fill()? {}
        let take = n.min(self.buf.len() - self.pos);
        let out = self.buf[self.pos..self.pos + take].to_vec();
        self.pos += take;
        Ok(out)
    }
    fn safe_read(&mut self, n: usize) -> Result<Vec<u8>, &'static str> {
        let data = self.read(n)?;
        if data.len() < n {
            Err("IncompleteRead")
        } else {
            Ok(data)
        }
    }
}
fn pending_owned(buf: &[u8], pos: usize) -> Vec<u8> {
    buf[pos..].to_vec()
}

fn is_py_space(c: char) -> bool {
    validate::python_whitespace(c)
}
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

fn read_status(r: &mut Reader) -> Result<(String, i128), &'static str> {
    let line = r.readline(MAX_LINE + 1)?;
    if line.len() > MAX_LINE {
        return Err("LineTooLong");
    }
    if line.is_empty() {
        return Err("RemoteDisconnected");
    }
    let text = latin1(&line);
    let mut words = text.split(is_py_space).filter(|w| !w.is_empty());
    let (Some(version), Some(status)) = (words.next(), words.next()) else {
        return Err("BadStatusLine");
    };
    if !version.starts_with("HTTP/") {
        return Err("BadStatusLine");
    }
    let status = status
        .is_ascii()
        .then(|| py_int(status.as_bytes(), 10))
        .flatten()
        .ok_or("BadStatusLine")?;
    if !(100..=999).contains(&status) {
        return Err("BadStatusLine");
    }
    Ok((version.to_owned(), status))
}

fn read_header_block(r: &mut Reader) -> Result<Vec<u8>, &'static str> {
    let (mut block, mut count) = (Vec::new(), 0);
    loop {
        let line = r.readline(MAX_LINE + 1)?;
        if line.len() > MAX_LINE {
            return Err("LineTooLong");
        }
        count += 1;
        if count > MAX_HEADERS {
            return Err("HTTPException");
        }
        if matches!(line.as_slice(), b"\r\n" | b"\n" | b"") {
            return Ok(block);
        }
        block.extend_from_slice(&line);
    }
}

/// Header fields as the `email` compat32 parser used by `http.client` reads them.
fn parse_headers(block: &[u8]) -> Vec<(String, String)> {
    let text = latin1(block);
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                lines.push(&text[start..i + 2]);
                i += 2;
                start = i;
            }
            b'\r' | b'\n' => {
                lines.push(&text[start..i + 1]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    let matches = |line: &str| {
        line.starts_with("From ")
            || line.starts_with([' ', '\t'])
            || line
                .find(|c: char| c == ':' || !('\x21'..='\x7e').contains(&c))
                .is_some_and(|i| line[i..].starts_with(':'))
    };
    let mut out = Vec::new();
    let mut last: Option<(String, String)> = None;
    for line in lines.into_iter().take_while(|l| matches(l)) {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = last.as_mut() {
                value.push_str(line);
            }
            continue;
        }
        if let Some((name, value)) = last.take() {
            out.push((name, value.trim_end_matches(['\r', '\n']).to_owned()));
        }
        if line.starts_with("From ") {
            continue;
        }
        match line.find(':') {
            Some(i) if i > 0 => {
                last = Some((
                    line[..i].to_owned(),
                    line[i + 1..].trim_start_matches([' ', '\t']).to_owned(),
                ))
            }
            _ => continue,
        }
    }
    if let Some((name, value)) = last {
        out.push((name, value.trim_end_matches(['\r', '\n']).to_owned()));
    }
    out
}
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn read_chunked(r: &mut Reader, mut amt: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    let mut left: Option<usize> = None;
    loop {
        let chunk = match left {
            Some(n) if n > 0 => n,
            _ => {
                if left.is_some() {
                    r.safe_read(2)?;
                }
                let line = r.readline(MAX_LINE + 1)?;
                if line.len() > MAX_LINE {
                    return Err("LineTooLong");
                }
                let size = line.split(|b| *b == b';').next().unwrap_or_default();
                let size = py_int(size, 16)
                    .filter(|n| *n >= 0)
                    .ok_or("IncompleteRead")?;
                if size == 0 {
                    loop {
                        let line = r.readline(MAX_LINE + 1)?;
                        if line.len() > MAX_LINE {
                            return Err("LineTooLong");
                        }
                        if matches!(line.as_slice(), b"\r\n" | b"\n" | b"") {
                            return Ok(out);
                        }
                    }
                }
                usize::try_from(size).unwrap_or(usize::MAX)
            }
        };
        if amt <= chunk {
            out.extend(r.safe_read(amt)?);
            return Ok(out);
        }
        out.extend(r.safe_read(chunk)?);
        amt -= chunk;
        left = Some(0);
    }
}

/// Status and the first MAX_RESPONSE + 1 body bytes, as `getresponse().read(MAX_RESPONSE + 1)`.
fn read_response(r: &mut Reader) -> Result<(i128, Vec<u8>), &'static str> {
    let (version, status) = loop {
        let (version, status) = read_status(r)?;
        if status != 100 {
            break (version, status);
        }
        read_header_block(r)?;
    };
    if !(version == "HTTP/1.0" || version == "HTTP/0.9" || version.starts_with("HTTP/1.")) {
        return Err("UnknownProtocol");
    }
    let headers = parse_headers(&read_header_block(r)?);
    let chunked =
        header(&headers, "transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked"));
    let mut length = if chunked {
        None
    } else {
        header(&headers, "content-length")
            .filter(|v| !v.is_empty())
            .and_then(|v| {
                if v.is_ascii() {
                    py_int(v.as_bytes(), 10)
                } else {
                    None
                }
            })
            .filter(|n| *n >= 0)
    };
    if status == 204 || status == 304 || (100..200).contains(&status) {
        length = Some(0);
    }
    let amt = MAX_RESPONSE + 1;
    let body = match length {
        Some(0) => Vec::new(),
        _ if chunked => read_chunked(r, amt)?,
        Some(n) => r.read(usize::try_from(n).unwrap_or(usize::MAX).min(amt))?,
        None => r.read(amt)?,
    };
    Ok((status, body))
}

struct Server {
    url: String,
    https: bool,
    host: String,
    port: u16,
    path: String,
}
impl Server {
    fn new(url: Option<&str>) -> Result<Self, Failure> {
        let url = validate::opencode_url(url).map_err(from_error)?;
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| coded("invalid_opencode_url"))?;
        let netloc_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (netloc, path) = rest.split_at(netloc_end);
        if netloc.contains('@') || path.contains(['?', '#']) {
            return Err(coded("invalid_opencode_url"));
        }
        // The same split as urllib.parse.urlsplit's hostname and port.
        let (host, port) = match netloc.split_once('[') {
            Some((_, bracketed)) => {
                let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                (host, after.split_once(':').map_or("", |p| p.1))
            }
            None => netloc.split_once(':').unwrap_or((netloc, "")),
        };
        let host = host.to_lowercase();
        if host.is_empty() {
            return Err(coded("invalid_opencode_url"));
        }
        if host != "localhost" && !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
            return Err(coded("opencode_url_must_be_loopback"));
        }
        let https = scheme == "https";
        let port = if port.is_empty() {
            if https { 443 } else { 80 }
        } else {
            if !port.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Failure::Class("ValueError"));
            }
            port.parse::<u16>()
                .map_err(|_| Failure::Class("ValueError"))?
        };
        Ok(Self {
            https,
            host,
            port,
            path: path.to_owned(),
            url,
        })
    }

    fn connect(&self) -> Result<Conn, Failure> {
        let unreachable = || coded("opencode_unreachable");
        let addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|_| unreachable())?;
        let stream = addrs
            .into_iter()
            .find_map(|addr| TcpStream::connect_timeout(&addr, TIMEOUT).ok())
            .ok_or_else(unreachable)?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|_| unreachable())?;
        let _ = stream.set_nodelay(true);
        if self.https {
            tls(stream, &self.host, self.port).map_err(|_| unreachable())
        } else {
            Ok(Conn::Plain(stream))
        }
    }

    /// Return (status, json); JSON null and an empty body are both None.
    fn request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: Option<&str>,
    ) -> Result<(i128, Option<Value>), Req> {
        let target = format!(
            "{}{}{}",
            self.path,
            path,
            query
                .map(|q| format!("?directory={}", quote(q, true)))
                .unwrap_or_default()
        );
        let auth = credentials().map_err(Req::Before)?;
        let payload = body.map(|text| {
            format!(
                "{{\"parts\": [{{\"type\": \"text\", \"text\": {}}}]}}",
                json_string(text)
            )
        });
        let conn = self.connect().map_err(Req::Before)?;
        if target.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(uncertain("InvalidURL"));
        }
        if !target.is_ascii() {
            return Err(Req::Before(Failure::Class("UnicodeEncodeError")));
        }
        let default_port = if self.https { 443 } else { 80 };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let host = if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        };
        let mut message =
            format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: identity\r\n");
        if let Some(payload) = &payload {
            message.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        message.push_str("Accept: application/json\r\n");
        if let Some(auth) = &auth {
            message.push_str(&format!("Authorization: {auth}\r\n"));
        }
        if payload.is_some() {
            message.push_str("Content-Type: application/json\r\n");
        }
        message.push_str("\r\n");
        message.push_str(payload.as_deref().unwrap_or(""));
        let mut reader = Reader {
            conn,
            buf: Vec::new(),
            pos: 0,
            eof: false,
        };
        reader
            .conn
            .write_all(message.as_bytes())
            .and_then(|_| reader.conn.flush())
            .map_err(|e| uncertain(io_class(&e)))?;
        let (status, raw) = read_response(&mut reader).map_err(uncertain)?;
        if raw.len() > MAX_RESPONSE {
            return Err(Req::After(coded("opencode_response_too_large")));
        }
        let trimmed = raw.trim_ascii();
        if trimmed.is_empty() {
            return Ok((status, None));
        }
        let text = raw.strip_prefix(b"\xef\xbb\xbf".as_slice()).unwrap_or(&raw);
        let data: Value = serde_json::from_slice(text)
            .map_err(|_| Req::After(coded("opencode_invalid_response")))?;
        Ok((status, if data.is_null() { None } else { Some(data) }))
    }

    fn get(&self, path: &str, query: Option<&str>) -> Result<(i128, Option<Value>), Failure> {
        self.request("GET", path, query, None)
            .map_err(Failure::from)
    }
}

fn expect(response: (i128, Option<Value>), missing: &str) -> Result<Value, Failure> {
    match response {
        (401, _) => Err(coded("opencode_unauthorized")),
        (404, _) => Err(coded(missing)),
        (200, Some(data)) => Ok(data),
        (status, _) => Err(Failure::coded(format!("opencode_http_{status}"))),
    }
}
fn invalid() -> Failure {
    coded("opencode_invalid_response")
}

fn health(server: &Server) -> Result<String, Failure> {
    let data = expect(
        server.get("/global/health", None)?,
        "recipient_not_in_opencode_server",
    )?;
    match (data.get("healthy"), data.get("version")) {
        (Some(Value::Bool(true)), Some(Value::String(version))) => Ok(version.clone()),
        _ => Err(invalid()),
    }
}

fn same_directory(directory: Option<&Value>, workspace: &str) -> Result<bool, Failure> {
    let Some(Value::String(directory)) =
        directory.filter(|d| d.as_str().is_some_and(|s| !s.is_empty()))
    else {
        return Err(invalid());
    };
    Ok(directory == workspace || os::resolve(Path::new(directory)).to_str() == Some(workspace))
}

fn session_info(server: &Server, id: &str, workspace: &str) -> Result<Map<String, Value>, Failure> {
    let data = expect(
        server.get(&format!("/session/{}", quote(id, false)), Some(workspace))?,
        "recipient_not_in_opencode_server",
    )?;
    let Value::Object(data) = data else {
        return Err(invalid());
    };
    let Some(Value::Object(time)) = data
        .get("time")
        .filter(|_| data.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Err(invalid());
    };
    if !same_directory(data.get("directory"), workspace)? {
        return Err(coded("recipient_identity_changed"));
    }
    if time.get("archived").is_some_and(|a| !a.is_null()) {
        return Err(coded("recipient_session_archived"));
    }
    Ok(data)
}

fn status_map(server: &Server, workspace: Option<&str>) -> Result<Map<String, Value>, Failure> {
    match expect(
        server.get("/session/status", workspace.filter(|w| !w.is_empty()))?,
        "recipient_not_in_opencode_server",
    )? {
        Value::Object(map) => Ok(map),
        _ => Err(invalid()),
    }
}

fn runtime_status(statuses: &Map<String, Value>, id: &str) -> Result<&'static str, Failure> {
    // The server omits idle sessions from this map.
    let Some(entry) = statuses.get(id).filter(|e| !e.is_null()) else {
        return Ok("idle");
    };
    let kind = entry
        .as_object()
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str);
    STATUSES
        .into_iter()
        .find(|s| Some(*s) == kind)
        .ok_or_else(invalid)
}

/// Exact session and workspace on the registered server, without messaging.
pub fn probe(peer: &Peer) -> Result<Value, Failure> {
    let server = Server::new(peer.url.as_deref())?;
    let version = health(&server)?;
    let id = session_id(&peer.session_id)?;
    let info = session_info(&server, &id, &peer.workspace)?;
    let runtime = runtime_status(&status_map(&server, Some(&peer.workspace))?, &id)?;
    Ok(
        json!({"harness": "opencode", "session_id": info.get("id"), "workspace": peer.workspace, "url": server.url,
        "server_version": version, "runtime_status": runtime, "transport": "opencode_http_prompt_async",
        "authenticated": credentials()?.is_some()}),
    )
}

/// One prompt_async attempt after preflight. A 204 proves acceptance only;
/// the model reading the text is established later by a reply or ack.
pub fn notify(peer: &Peer, body: &str, skip: Skip<'_>) -> Outcome {
    let preflight = (|| {
        let server = Server::new(peer.url.as_deref())?;
        probe(peer)?;
        Ok::<_, Failure>((server, skip()?))
    })();
    let server = match preflight {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok((_, Some(reason))) => return Outcome::not_submitted(reason.as_str()),
        Ok((server, None)) => server,
    };
    let path = format!("/session/{}/prompt_async", quote(&peer.session_id, false));
    match server.request("POST", &path, Some(&peer.workspace), Some(body)) {
        Err(Req::Before(failure)) => Outcome::not_submitted(failure.to_string()),
        Err(Req::After(failure)) => Outcome::unknown(failure.to_string()),
        Ok((204, _)) => Outcome::submitted("opencode_prompt_async_accepted"),
        Ok((401, _)) => Outcome::not_submitted("opencode_unauthorized"),
        Ok((404, _)) => Outcome::not_submitted("recipient_not_in_opencode_server"),
        Ok((status @ 400..=499, _)) => Outcome::not_submitted(format!("opencode_http_{status}")),
        Ok((status, _)) => Outcome::unknown(format!("opencode_http_{status}")),
    }
}

fn saved_database() -> PathBuf {
    let base = env::var_os("XDG_DATA_HOME")
        .filter(|b| !b.is_empty())
        .map_or_else(|| os::home().join(".local/share"), PathBuf::from);
    os::expand_user(&base).join("opencode/opencode.db")
}

/// `str(pathlib.Path(p))`: repeated slashes and `.` parts removed.
fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let body = text
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/");
    if root.is_empty() && body.is_empty() {
        ".".to_owned()
    } else {
        format!("{root}{body}")
    }
}

fn candidate(
    session_id: &str,
    directory: &str,
    runtime: &str,
    reason: &str,
    source: &str,
    url: Option<&str>,
    updated: Value,
) -> Value {
    json!({"harness": "opencode", "session_id": session_id, "workspace": directory, "runtime_status": runtime,
        "runtime_reason": reason, "source": source, "url": url, "updated_at": updated})
}

fn reject(source: &mut Value, detail: Option<&str>) {
    let rejected = source.get("rejected").and_then(Value::as_i64).unwrap_or(0) + 1;
    source["status"] = json!("partial");
    if let Some(detail) = detail {
        source["detail"] = json!(detail);
    }
    source["rejected"] = json!(rejected);
}

/// Sessions of the requested project, or the server's own; GET only.
fn server_sessions(url: &str, workspace: Option<&str>) -> (Vec<Value>, Value) {
    let mut source = json!({"harness": "opencode", "source": "opencode_server", "url": null, "status": "ok",
        "version": null, "error": null, "detail": null});
    let mut found = Vec::new();
    let result = (|| {
        let server = Server::new(Some(url))?; // Never echo a malformed URL: it may carry userinfo.
        source["url"] = json!(server.url);
        source["version"] = json!(health(&server)?);
        let scope = workspace.filter(|w| !w.is_empty());
        let listed = expect(
            server.get("/session", scope)?,
            "recipient_not_in_opencode_server",
        )?;
        let statuses = status_map(&server, workspace)?;
        let Value::Array(listed) = listed else {
            return Err(invalid());
        };
        for item in &listed {
            let row = (|| {
                let time = item
                    .get("time")
                    .and_then(Value::as_object)
                    .filter(|_| item.is_object())
                    .ok_or(())?;
                if truthy(item.get("parentID"))
                    || time.get("archived").is_some_and(|a| !a.is_null())
                {
                    return Ok(None);
                }
                let directory = item
                    .get("directory")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                    .ok_or(())?;
                let id = item.get("id").and_then(Value::as_str).ok_or(())?;
                let id = session_id(id).map_err(|_| ())?;
                let runtime = runtime_status(&statuses, &id).map_err(|_| ())?;
                Ok(Some(candidate(
                    &id,
                    directory,
                    runtime,
                    "server_status",
                    "opencode_server",
                    Some(&server.url),
                    seconds(time.get("updated")),
                )))
            })();
            match row {
                Ok(Some(row)) => found.push(row),
                Ok(None) => (),
                Err(()) => reject(&mut source, Some("opencode_invalid_session_records")),
            }
        }
        Ok::<_, Failure>(())
    })();
    if let Err(failure) = result {
        source["status"] = json!("unavailable");
        source["error"] = json!(match failure {
            Failure::Coded(code) => code,
            Failure::Class(name) => format!("opencode_{name}"),
        });
        found.clear();
    }
    (found, source)
}

enum Cell {
    Int(i64),
    Text(String),
    Other,
}

/// Unarchived root sessions from the local SQLite metadata, read-only. Liveness is unknown.
fn saved_sessions(path: &Path) -> (Vec<Value>, Value) {
    let mut source = json!({"harness": "opencode", "source": "opencode_saved", "path": path_text(path), "status": "ok",
        "error": null, "detail": null});
    let mut found = Vec::new();
    if !path.is_file() {
        source["status"] = json!("unavailable");
        source["error"] = json!("opencode_saved_metadata_missing");
        return (found, source);
    }
    let rows = (|| {
        let db = rusqlite::Connection::open_with_flags(
            os::resolve(path),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_secs(3))?;
        let mut statement = db.prepare(
            "SELECT id, directory, time_updated FROM session
            WHERE parent_id IS NULL AND time_archived IS NULL
            ORDER BY time_updated DESC LIMIT ?",
        )?;
        let cell = |value: ValueRef<'_>| -> Result<Cell, Failure> {
            Ok(match value {
                ValueRef::Integer(v) => Cell::Int(v),
                ValueRef::Text(t) => Cell::Text(
                    std::str::from_utf8(t)
                        .map_err(|_| Failure::Class("OperationalError"))?
                        .to_owned(),
                ),
                _ => Cell::Other,
            })
        };
        let mut rows = statement.query([SAVED_LIMIT as i64 + 1])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((
                cell(row.get_ref(0)?)?,
                cell(row.get_ref(1)?)?,
                cell(row.get_ref(2)?)?,
            ));
        }
        Ok::<_, Failure>(out)
    })();
    let mut rows = match rows {
        Ok(rows) => rows,
        Err(failure) => {
            source["status"] = json!("unavailable");
            source["detail"] = Value::Null;
            source["error"] = json!(match failure {
                Failure::Coded(code) => code,
                Failure::Class(name) => format!("opencode_saved_{name}"),
            });
            return (Vec::new(), source);
        }
    };
    if rows.len() > SAVED_LIMIT {
        rows.truncate(SAVED_LIMIT);
        source["status"] = json!("partial");
        source["detail"] = json!("opencode_saved_session_limit_reached");
    }
    for (id, directory, updated) in rows {
        let row = match (&id, &directory) {
            (Cell::Text(id), Cell::Text(directory)) if !directory.is_empty() => {
                session_id(id).ok().map(|id| {
                    let updated = match updated {
                        Cell::Int(v) => json!(v.div_euclid(1000)),
                        _ => Value::Null,
                    };
                    candidate(
                        &id,
                        directory,
                        "unknown",
                        "saved_metadata_only",
                        "opencode_saved",
                        None,
                        updated,
                    )
                })
            }
            _ => None,
        };
        match row {
            Some(row) => found.push(row),
            None => {
                let keep = source["detail"].as_str().map(str::to_owned);
                reject(
                    &mut source,
                    Some(keep.as_deref().unwrap_or("opencode_invalid_saved_metadata")),
                );
            }
        }
    }
    (found, source)
}

/// Read-only discovery: GET requests and a read-only metadata file only; no
/// session is created and no turn starts. A malformed URL is confined to its
/// own source entry.
pub fn discover(urls: Option<&[String]>, workspace: Option<&str>) -> Found {
    discover_with(urls, workspace, None)
}

/// `discover` with an explicit saved-metadata database instead of the XDG default.
pub fn discover_with(
    urls: Option<&[String]>,
    workspace: Option<&str>,
    database: Option<&Path>,
) -> Found {
    let default = [DEFAULT_URL.to_owned()];
    let mut found = Found::default();
    let mut seen = std::collections::HashSet::new();
    for url in urls.unwrap_or(&default) {
        let (sessions, source) = server_sessions(url, workspace);
        found.sources.push(source);
        for item in sessions {
            if seen.insert(item["session_id"].as_str().unwrap_or_default().to_owned()) {
                found.sessions.push(item);
            }
        }
    }
    let database = database
        .filter(|d| !d.as_os_str().is_empty())
        .map_or_else(saved_database, Path::to_path_buf);
    let (sessions, source) = saved_sessions(&database);
    found.sources.push(source);
    found.sessions.extend(
        sessions
            .into_iter()
            .filter(|item| !seen.contains(item["session_id"].as_str().unwrap_or_default())),
    );
    found
}
