use crate::error::Error;
use std::net::IpAddr;

pub fn uuid(value: &str) -> Result<String, Error> {
    let hex = value.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(['{', '}']).replace('-', "");
    uuid::Uuid::parse_str(&hex)
        .map(|id| id.to_string())
        .map_err(|_| Error::code("badly formed hexadecimal UUID string"))
}
pub fn peer_name(value: &str) -> Result<(), Error> {
    let b = value.as_bytes();
    if b.is_empty()
        || b.len() > 64
        || !b[0].is_ascii_lowercase() && !b[0].is_ascii_digit()
        || !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
    {
        return Err(Error::code("invalid_peer_name"));
    }
    Ok(())
}
pub fn opencode_session_id(value: &str) -> Result<String, Error> {
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
pub fn opencode_url(value: Option<&str>) -> Result<String, Error> {
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
            return Err(Error::code("Port could not be cast to integer value"));
        }
        port.parse::<u16>()
            .map_err(|_| Error::code("Port out of range 0-65535"))?;
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

pub fn text(body: &str) -> Result<(), Error> {
    if body.trim_matches(python_whitespace).is_empty() || body.len() > 32000 {
        Err(Error::code("message_must_be_1_to_32000_bytes"))
    } else {
        Ok(())
    }
}
pub fn wait(seconds: f64) -> Result<(), Error> {
    if !seconds.is_finite() || !(0.0..=45.0).contains(&seconds) {
        Err(Error::code("wait_seconds_must_be_between_0_and_45"))
    } else {
        Ok(())
    }
}
pub fn page(limit: i64, cursor: Option<i64>) -> Result<(), Error> {
    if !(1..=500).contains(&limit) {
        return Err(Error::code("limit_must_be_between_1_and_500"));
    }
    if cursor.is_some_and(|c| c < 1) {
        return Err(Error::code("seq_cursor_must_be_a_positive_integer"));
    }
    Ok(())
}
pub fn python_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}
pub fn preview(body: &str) -> String {
    body.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ])
    .map(|line| line.trim_matches(python_whitespace))
    .find(|line| !line.is_empty())
    .unwrap_or("")
    .chars()
    .take(120)
    .collect()
}
