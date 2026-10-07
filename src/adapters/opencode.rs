//! OpenCode sessions through the official local HTTP server API.
//!
//! Discovery and checks use GET only and never create a session. Delivery is one
//! POST /session/{id}/prompt_async, which intentionally starts a turn in the
//! existing session. No credential is stored.
//!
//! Each request is its own connection, with no redirect, no proxy and no second attempt,
//! 5 s for each step of the exchange and at most 4 MiB of response body. A prompt counts as
//! not sent only while none of its body was handed over; every later failure is uncertain.
use super::{Adapter, Address, Query};
use crate::model::NativePeer as Peer;
use crate::{
    error::{Error, io_code},
    model::*,
    os,
};
use base64::Engine;
use rusqlite::{OpenFlags, types::ValueRef};
use serde_json::{Map, Value, json};
use std::{
    cell::Cell,
    env, fs,
    io::{self, Read},
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const DEFAULT_URL: &str = "http://127.0.0.1:4096";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const SAVED_LIMIT: usize = 50;
const STATUSES: [&str; 3] = ["idle", "busy", "retry"];

/// A request failure; `After` means the request may have reached the server.
enum Req {
    Before(Error),
    After(Error),
}
impl From<Req> for Error {
    fn from(r: Req) -> Self {
        match r {
            Req::Before(f) | Req::After(f) => f,
        }
    }
}
fn session_id(value: &str) -> Result<String, Error> {
    opencode_session_id(value)
}
fn opencode_session_id(value: &str) -> Result<String, Error> {
    if !value.starts_with("ses")
        || value.len() < 4
        || value.len() > 256
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(Error::code("invalid_opencode_session_id"));
    }
    Ok(value.to_owned())
}
fn opencode_url(value: Option<&str>) -> Result<String, Error> {
    // url::Url uses browser-style host normalization. Check the supplied host
    // first so shorthand, percent-encoded and IDNA lookalikes stay rejected.
    let value = value
        .unwrap_or("http://127.0.0.1:4096")
        .trim_start_matches(|c: char| c <= ' ')
        .replace(['\t', '\r', '\n'], "");
    let invalid = || Error::code("invalid_opencode_url");
    let (scheme, rest) = value.split_once("://").ok_or_else(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") || authority.contains('@') {
        return Err(invalid());
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or_else(invalid)?;
        (host, after.strip_prefix(':').unwrap_or(""))
    } else {
        authority.split_once(':').unwrap_or((authority, ""))
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return Err(invalid());
    }
    if host != "localhost" && !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return Err(Error::code("opencode_url_must_be_loopback"));
    }
    if !port.is_empty() {
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        port.parse::<u16>().map_err(|_| invalid())?;
    }
    let parsed = url::Url::parse(&value).map_err(|_| invalid())?;
    if parsed.query().is_some_and(|q| !q.is_empty())
        || parsed.fragment().is_some_and(|f| !f.is_empty())
    {
        return Err(invalid());
    }
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/');
    Ok(format!("{scheme}://{rest}"))
}

/// Basic auth from the same environment the server reads; never persisted.
fn credentials() -> Result<Option<String>, Error> {
    let password = env::var_os("OPENCODE_SERVER_PASSWORD").filter(|p| !p.is_empty());
    let Some(password) = password else {
        return Ok(None);
    };
    let user = env::var_os("OPENCODE_SERVER_USERNAME").filter(|u| !u.is_empty());
    let user = user.as_deref().map_or(Some("opencode"), |u| u.to_str());
    let (Some(user), Some(password)) = (user, password.to_str()) else {
        return Err(Error::code("invalid_opencode_credentials"));
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

/// What a failed request is called after `opencode_`.
fn class(error: &ureq::Error) -> &'static str {
    use io::ErrorKind::*;
    use ureq::{Error as E, Timeout as T};
    match error {
        E::Timeout(T::Resolve | T::Connect) | E::HostNotFound | E::ConnectionFailed => {
            "unreachable"
        }
        E::Timeout(_) => "timed_out",
        E::Io(_) if os::interrupted() => "interrupted",
        E::Io(e) => match e.kind() {
            ConnectionRefused | AddrNotAvailable | NetworkUnreachable | HostUnreachable => {
                "unreachable"
            }
            UnexpectedEof => "connection_closed",
            // What rustls makes of a certificate it does not trust or a broken record.
            InvalidData => "tls_failed",
            kind => io_code(kind),
        },
        E::Rustls(_) | E::Tls(_) => "tls_failed",
        _ => "invalid_response",
    }
}

/// A prompt's body, which knows whether any of it was asked for. Until then the server has
/// nothing it could act on.
struct Prompt<'a> {
    left: &'a [u8],
    asked: &'a Cell<bool>,
}
impl Read for Prompt<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // The last moment an interrupt still keeps the prompt from the server.
        if !self.asked.get() && os::interrupted() {
            return Err(io::Error::other("interrupted"));
        }
        self.asked.set(true);
        self.left.read(buf)
    }
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

struct Server {
    url: String,
    https: bool,
    host: String,
    port: u16,
    path: String,
}
impl Server {
    fn new(url: Option<&str>) -> Result<Self, Error> {
        let url = opencode_url(url)?;
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| Error::code("invalid_opencode_url"))?;
        let netloc_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (netloc, path) = rest.split_at(netloc_end);
        if netloc.contains('@') || path.contains(['?', '#']) {
            return Err(Error::code("invalid_opencode_url"));
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
            return Err(Error::code("invalid_opencode_url"));
        }
        if host != "localhost" && !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
            return Err(Error::code("opencode_url_must_be_loopback"));
        }
        let https = scheme == "https";
        let port = if port.is_empty() {
            if https { 443 } else { 80 }
        } else {
            if !port.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::code("invalid_opencode_url"));
            }
            port.parse::<u16>()
                .map_err(|_| Error::code("invalid_opencode_url"))?
        };
        Ok(Self {
            https,
            host,
            port,
            path: path.to_owned(),
            url,
        })
    }

    /// Where a request may go. Plain HTTP goes to an address of this machine by its number,
    /// so no answer of a resolver widens it. HTTPS needs the name for the certificate, and
    /// the certificate is then what holds it to this machine.
    fn authorities(&self, name: &str) -> Vec<String> {
        let Ok(found) = (self.host.as_str(), self.port).to_socket_addrs() else {
            return Vec::new();
        };
        let (local, outside): (Vec<_>, Vec<_>) = found.partition(|a| a.ip().is_loopback());
        if !self.https {
            return local.iter().map(ToString::to_string).collect();
        }
        if local.is_empty() || !outside.is_empty() {
            return Vec::new();
        }
        vec![name.to_owned()]
    }

    /// One exchange with one address: the status and the whole body.
    fn exchange(
        &self,
        uri: String,
        name: &str,
        auth: Option<&str>,
        payload: Option<&str>,
    ) -> Result<(u16, Vec<u8>), Req> {
        use ureq::tls::{RootCerts, TlsConfig};
        let mut config = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(ureq::config::AutoHeaderValue::None)
            .timeout_connect(Some(TIMEOUT))
            .timeout_send_request(Some(TIMEOUT))
            .timeout_send_body(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            .timeout_recv_body(Some(TIMEOUT));
        if self.https {
            let roots = RootCerts::Specific(Arc::new(trust_roots()));
            config = config.tls_config(TlsConfig::builder().root_certs(roots).build());
        }
        let agent: ureq::Agent = config.build().into();
        let mut request = ureq::http::Request::builder()
            .method(if payload.is_some() { "POST" } else { "GET" })
            .uri(uri)
            .header("Host", name)
            .header("Accept-Encoding", "identity")
            .header("Accept", "application/json");
        if let Some(auth) = auth {
            request = request.header("Authorization", auth);
        }
        if let Some(payload) = payload {
            request = request
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
        }
        let asked = Cell::new(false);
        let mut prompt = Prompt {
            left: payload.unwrap_or_default().as_bytes(),
            asked: &asked,
        };
        let sent = match payload {
            Some(_) => request
                .body(ureq::SendBody::from_reader(&mut prompt))
                .map(|request| agent.run(request)),
            None => request.body(()).map(|request| agent.run(request)),
        };
        let failed = |error: ureq::Error| {
            let coded = |class| Error::code(format!("opencode_{class}"));
            if asked.get() {
                return Req::After(coded(class(&error)));
            }
            Req::Before(coded(match class(&error) {
                // A handshake that failed reached no server.
                "tls_failed" => "unreachable",
                class => class,
            }))
        };
        let mut answer = sent
            .map_err(|_| Req::Before(Error::code("invalid_opencode_url")))?
            .map_err(failed)?;
        let status = answer.status().as_u16();
        let mut whole = answer.body_mut().as_reader();
        let (mut raw, mut part) = (Vec::new(), [0; 8192]);
        // Not `read_to_end`, which starts a read again after an interrupt ended it.
        while raw.len() <= MAX_RESPONSE {
            match whole.read(&mut part) {
                Ok(0) => return Ok((status, raw)),
                Ok(n) => raw.extend_from_slice(&part[..n]),
                Err(e) => return Err(failed(e.into())),
            }
        }
        Err(Req::After(Error::code("opencode_response_too_large")))
    }

    /// Return (status, json); JSON null and an empty body are both None.
    fn request(
        &self,
        path: &str,
        query: Option<&str>,
        body: Option<&str>,
    ) -> Result<(u16, Option<Value>), Req> {
        let unreachable = || Error::code("opencode_unreachable");
        let auth = credentials().map_err(Req::Before)?;
        let payload =
            body.map(|text| json!({"parts": [{"type": "text", "text": text}]}).to_string());
        let scheme = if self.https { "https" } else { "http" };
        let name = match self.host.contains(':') {
            true => format!("[{}]:{}", self.host, self.port),
            false => format!("{}:{}", self.host, self.port),
        };
        let query = query
            .map(|q| format!("?directory={}", quote(q, true)))
            .unwrap_or_default();
        let mut tried = Err(Req::Before(unreachable()));
        for authority in self.authorities(&name) {
            let uri = format!("{scheme}://{authority}{}{path}{query}", self.path);
            tried = self.exchange(uri, &name, auth.as_deref(), payload.as_deref());
            // An address that took no connection was sent nothing, so the next one is no
            // second attempt.
            if !matches!(&tried, Err(Req::Before(e)) if *e == unreachable()) {
                break;
            }
        }
        let (status, raw) = tried?;
        // A completely framed rejection is meaningful even when its error body is not JSON.
        if !(200..300).contains(&status) || raw.trim_ascii().is_empty() {
            return Ok((status, None));
        }
        let text = raw.strip_prefix(b"\xef\xbb\xbf".as_slice()).unwrap_or(&raw);
        let data: Value = serde_json::from_slice(text)
            .map_err(|_| Req::After(Error::code("opencode_invalid_response")))?;
        Ok((status, if data.is_null() { None } else { Some(data) }))
    }

    fn get(&self, path: &str, query: Option<&str>) -> Result<(u16, Option<Value>), Error> {
        self.request(path, query, None).map_err(Error::from)
    }
}

fn expect(response: (u16, Option<Value>), missing: &'static str) -> Result<Value, Error> {
    match response {
        (401, _) => Err(Error::code("opencode_unauthorized")),
        (404, _) => Err(Error::code(missing)),
        (200, Some(data)) => Ok(data),
        (status, _) => Err(Error::code(format!("opencode_http_{status}"))),
    }
}
fn invalid() -> Error {
    Error::code("opencode_invalid_response")
}

fn health(server: &Server) -> Result<String, Error> {
    let data = expect(
        server.get("/global/health", None)?,
        "recipient_not_in_opencode_server",
    )?;
    match (data.get("healthy"), data.get("version")) {
        (Some(Value::Bool(true)), Some(Value::String(version))) => Ok(version.clone()),
        _ => Err(invalid()),
    }
}

fn same_directory(directory: Option<&Value>, workspace: &str) -> Result<bool, Error> {
    let Some(Value::String(directory)) =
        directory.filter(|d| d.as_str().is_some_and(|s| !s.is_empty()))
    else {
        return Err(invalid());
    };
    Ok(directory == workspace
        || os::same_path(
            &os::resolve(Path::new(directory)).to_string_lossy(),
            workspace,
        ))
}

fn session_info(server: &Server, id: &str, workspace: &str) -> Result<Map<String, Value>, Error> {
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
        return Err(Error::code("recipient_identity_changed"));
    }
    if time.get("archived").is_some_and(|a| !a.is_null()) {
        return Err(Error::code("recipient_session_archived"));
    }
    Ok(data)
}

fn status_map(server: &Server, workspace: Option<&str>) -> Result<Map<String, Value>, Error> {
    match expect(
        server.get("/session/status", workspace.filter(|w| !w.is_empty()))?,
        "recipient_not_in_opencode_server",
    )? {
        Value::Object(map) => Ok(map),
        _ => Err(invalid()),
    }
}

fn runtime_status(statuses: &Map<String, Value>, id: &str) -> Result<&'static str, Error> {
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

pub(crate) struct Opencode;
impl Adapter for Opencode {
    // OpenCode identifiers are opaque, and the address is a server URL, never a socket.
    fn address(
        &self,
        session: &str,
        workspace: &str,
        socket: Option<&str>,
        url: Option<&str>,
    ) -> Result<Address, Error> {
        let session_id = opencode_session_id(session)?;
        if socket.is_some() {
            return Err(Error::code("opencode_uses_a_server_url_not_a_socket"));
        }
        let url = Some(opencode_url(url)?);
        Ok(Address {
            session_id,
            workspace: super::workspace(workspace)?,
            socket: None,
            url,
        })
    }
    fn notify(&self, peer: &Peer, _message: &Message, body: &str, skip: Skip<'_>) -> Outcome {
        notify(peer, body, skip)
    }
    fn probe(&self, peer: &Peer) -> Result<Value, Error> {
        probe(peer)
    }
    fn discover(&self, query: &Query<'_>) -> Found {
        discover(query.opencode_urls, query.workspace)
    }
}

/// Exact session and workspace on the registered server, without messaging.
fn probe(peer: &Peer) -> Result<Value, Error> {
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
fn notify(peer: &Peer, body: &str, skip: Skip<'_>) -> Outcome {
    let preflight = (|| {
        let server = Server::new(peer.url.as_deref())?;
        probe(peer)?;
        Ok::<_, Error>((server, skip()?))
    })();
    let server = match preflight {
        Err(failure) => return Outcome::not_submitted(failure.to_string()),
        Ok((_, Some(reason))) => return Outcome::not_submitted(reason.as_str()),
        Ok((server, None)) => server,
    };
    if os::interrupted() {
        return Outcome::not_submitted(Error::Interrupted.to_string());
    }
    let path = format!("/session/{}/prompt_async", quote(&peer.session_id, false));
    match server.request(&path, Some(&peer.workspace), Some(body)) {
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
    os::expand_user(&os::data_home()).join("opencode/opencode.db")
}

/// `str(pathlib.Path(p))`: repeated slashes and `.` parts removed.
fn path_text(path: &Path) -> String {
    if cfg!(windows) {
        // Its parts again, joined by the separator of the system.
        return crate::discovery::lossy(&path.components().collect::<PathBuf>());
    }
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
        Ok::<_, Error>(())
    })();
    if let Err(failure) = result {
        source["status"] = json!("unavailable");
        source["error"] = json!(if failure.system() {
            format!("opencode_{failure}")
        } else {
            failure.to_string()
        });
        found.clear();
    }
    (found, source)
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
        let text = |value: ValueRef<'_>| match value {
            ValueRef::Text(t) => std::str::from_utf8(t).ok().map(str::to_owned),
            _ => None,
        };
        let mut rows = statement.query([SAVED_LIMIT as i64 + 1])?;
        let mut out = Vec::new();
        let mut limited = false;
        while let Some(row) = rows.next()? {
            // The extra row detects the limit; its cells are never interpreted.
            if out.len() == SAVED_LIMIT {
                limited = true;
                break;
            }
            // Timestamps are optional integers. Do not decode unused TEXT values.
            let updated = match row.get_ref(2)? {
                ValueRef::Integer(v) => Some(v),
                _ => None,
            };
            out.push((text(row.get_ref(0)?), text(row.get_ref(1)?), updated));
        }
        Ok::<_, Error>((out, limited))
    })();
    let (rows, limited) = match rows {
        Ok(rows) => rows,
        Err(failure) => {
            source["status"] = json!("unavailable");
            source["detail"] = Value::Null;
            source["error"] = json!(if failure.system() {
                format!("opencode_saved_{failure}")
            } else {
                failure.to_string()
            });
            return (Vec::new(), source);
        }
    };
    if limited {
        source["status"] = json!("partial");
        source["detail"] = json!("opencode_saved_session_limit_reached");
    }
    for (id, directory, updated) in rows {
        let row = match (&id, &directory) {
            (Some(id), Some(directory)) if !directory.is_empty() => session_id(id).ok().map(|id| {
                let updated = updated.map_or(Value::Null, |v| json!(v.div_euclid(1000)));
                candidate(
                    &id,
                    directory,
                    "unknown",
                    "saved_metadata_only",
                    "opencode_saved",
                    None,
                    updated,
                )
            }),
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
fn discover(urls: Option<&[String]>, workspace: Option<&str>) -> Found {
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
    let (sessions, source) = saved_sessions(&saved_database());
    found.sources.push(source);
    found.sessions.extend(
        sessions
            .into_iter()
            .filter(|item| !seen.contains(item["session_id"].as_str().unwrap_or_default())),
    );
    found
}
