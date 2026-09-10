"""OpenCode sessions through the official local HTTP server API.

Discovery and checks use GET only and never create a session. Delivery is one
POST /session/{id}/prompt_async, which intentionally starts a turn in the
existing session. No credential is stored.
"""
from base64 import b64encode
from contextlib import closing
import http.client
import ipaddress
import json
import os
from pathlib import Path
import re
import sqlite3
from urllib.parse import quote, urlencode, urlsplit

DEFAULT_URL = "http://127.0.0.1:4096"
TIMEOUT = 5
MAX_RESPONSE = 4 * 1024 * 1024
SAVED_LIMIT = 50
STATUSES = ("idle", "busy", "retry")


class OpenCodeError(ValueError):
    """A fixed code; never server prose or credentials."""


class Uncertain(OSError):
    """The request may have reached the server before this failure."""


def valid_session_id(value):
    if not isinstance(value, str) or not re.fullmatch(r"ses[A-Za-z0-9_.-]{1,253}", value):
        raise ValueError("invalid_opencode_session_id")
    return value


def valid_url(value):
    value = DEFAULT_URL if value is None else value
    parts = urlsplit(value) if isinstance(value, str) else None
    if (not parts or parts.scheme not in ("http", "https") or not parts.hostname or parts.username is not None
            or parts.password is not None or parts.query or parts.fragment):
        raise ValueError("invalid_opencode_url")
    if parts.hostname != "localhost":
        try:
            loopback = ipaddress.ip_address(parts.hostname).is_loopback
        except ValueError:
            loopback = False  # Names such as 127.attacker.example are not addresses.
        if not loopback:
            raise ValueError("opencode_url_must_be_loopback")
    parts.port  # A malformed port raises here.
    return parts.scheme + "://" + parts.netloc + parts.path.rstrip("/")


def credentials():
    """Basic auth from the same environment the server reads; never persisted."""
    password = os.environ.get("OPENCODE_SERVER_PASSWORD")
    if not password:
        return None
    user = os.environ.get("OPENCODE_SERVER_USERNAME") or "opencode"
    return "Basic " + b64encode(f"{user}:{password}".encode()).decode()


class Server:
    def __init__(self, url=None, timeout=TIMEOUT):
        self.url = valid_url(url)
        self.parts = urlsplit(self.url)
        self.timeout = timeout

    def request(self, method, path, query=None, body=None):
        """Return (status, json). opencode_unreachable is raised only before the
        request is written; every later failure is Uncertain."""
        target = self.parts.path + path + ("?" + urlencode(query) if query else "")
        headers = {"Accept": "application/json"}
        auth = credentials()
        if auth:
            headers["Authorization"] = auth
        payload = None
        if body is not None:
            payload = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        kind = http.client.HTTPSConnection if self.parts.scheme == "https" else http.client.HTTPConnection
        connection = kind(self.parts.hostname, self.parts.port, timeout=self.timeout)
        try:
            try:
                connection.connect()
            except OSError as exc:
                raise OpenCodeError("opencode_unreachable") from exc
            try:
                connection.request(method, target, body=payload, headers=headers)
                response = connection.getresponse()
                status, raw = response.status, response.read(MAX_RESPONSE + 1)
            except (OSError, http.client.HTTPException) as exc:
                raise Uncertain("opencode_" + type(exc).__name__) from exc
        finally:
            connection.close()
        if len(raw) > MAX_RESPONSE:
            raise Uncertain("opencode_response_too_large")
        data = None
        if raw.strip():
            try:
                data = json.loads(raw)
            except ValueError:
                raise Uncertain("opencode_invalid_response") from None
        return status, data


def expect(status, data, *, missing="recipient_not_in_opencode_server"):
    if status == 401:
        raise OpenCodeError("opencode_unauthorized")
    if status == 404:
        raise OpenCodeError(missing)
    if status != 200 or data is None:
        raise OpenCodeError("opencode_http_%d" % status)
    return data


def health(server):
    data = expect(*server.request("GET", "/global/health"))
    if not isinstance(data, dict) or data.get("healthy") is not True or not isinstance(data.get("version"), str):
        raise OpenCodeError("opencode_invalid_response")
    return data["version"]


def same_directory(directory, workspace):
    if not isinstance(directory, str) or not directory:
        raise OpenCodeError("opencode_invalid_response")
    return directory == workspace or str(Path(directory).expanduser().resolve()) == workspace


def session_info(server, session_id, workspace):
    data = expect(*server.request("GET", "/session/" + quote(session_id, safe=""), {"directory": workspace}))
    if not isinstance(data, dict) or data.get("id") != session_id or not isinstance(data.get("time"), dict):
        raise OpenCodeError("opencode_invalid_response")
    if not same_directory(data.get("directory"), workspace):
        raise OpenCodeError("recipient_identity_changed")
    if data["time"].get("archived") is not None:
        raise OpenCodeError("recipient_session_archived")
    return data


def status_map(server, workspace=None):
    data = expect(*server.request("GET", "/session/status", {"directory": workspace} if workspace else None))
    if not isinstance(data, dict):
        raise OpenCodeError("opencode_invalid_response")
    return data


def runtime_status(statuses, session_id):
    entry = statuses.get(session_id)
    if entry is None:
        return "idle"  # The server omits idle sessions from this map.
    kind = entry.get("type") if isinstance(entry, dict) else None
    if kind not in STATUSES:
        raise OpenCodeError("opencode_invalid_response")
    return kind


def probe(peer):
    """Exact session and workspace on the registered server, without messaging."""
    server = Server(peer.get("url"))
    version = health(server)
    info = session_info(server, valid_session_id(peer["session_id"]), peer["workspace"])
    return {"harness": "opencode", "session_id": info["id"], "workspace": peer["workspace"], "url": server.url,
            "server_version": version, "runtime_status": runtime_status(status_map(server, peer["workspace"]), info["id"]),
            "transport": "opencode_http_prompt_async", "authenticated": credentials() is not None}


def notify(peer, body, *, still_needed=None):
    """One prompt_async attempt after preflight. A 2xx proves acceptance only;
    the model reading the text is established later by a reply or ack."""
    try:
        server = Server(peer.get("url"))
        probe(peer)
        if still_needed is not None and not still_needed():
            return "not_submitted", "acknowledged_before_notification"
    except (OSError, ValueError, KeyError, TypeError) as exc:
        return "not_submitted", str(exc) if isinstance(exc, OpenCodeError) else type(exc).__name__
    try:
        status, _ = server.request("POST", "/session/" + quote(peer["session_id"], safe="") + "/prompt_async",
                                   {"directory": peer["workspace"]}, {"parts": [{"type": "text", "text": body}]})
    except OpenCodeError as exc:
        return "not_submitted", str(exc)
    except Uncertain as exc:
        return "submission_unknown", str(exc)
    if status == 204:
        return "submitted", "opencode_prompt_async_accepted"
    if status == 401:
        return "not_submitted", "opencode_unauthorized"
    if status == 404:
        return "not_submitted", "recipient_not_in_opencode_server"
    if 400 <= status < 500:
        return "not_submitted", "opencode_http_%d" % status
    return "submission_unknown", "opencode_http_%d" % status


def saved_database():
    base = os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share"
    return Path(base).expanduser() / "opencode/opencode.db"


def candidate(session_id, directory, runtime, reason, source, url, updated):
    return {"harness": "opencode", "session_id": session_id, "workspace": directory, "runtime_status": runtime,
            "runtime_reason": reason, "source": source, "url": url, "updated_at": updated}


def server_sessions(url, workspace=None):
    """Sessions of the requested project, or the server's own; GET only."""
    source = {"harness": "opencode", "source": "opencode_server", "url": None, "status": "ok",
              "version": None, "error": None, "detail": None}
    found = []
    try:
        server = Server(url)  # Never echo a malformed URL: it may carry userinfo.
        source["url"] = server.url
        source["version"] = health(server)
        scope = {"directory": workspace} if workspace else None
        listed = expect(*server.request("GET", "/session", scope))
        statuses = status_map(server, workspace)
        if not isinstance(listed, list):
            raise OpenCodeError("opencode_invalid_response")
        for item in listed:
            try:
                if not isinstance(item, dict) or not isinstance(item.get("time"), dict):
                    raise OpenCodeError("opencode_invalid_response")
                if item.get("parentID") or item["time"].get("archived") is not None:
                    continue
                directory = item.get("directory")
                if not isinstance(directory, str) or not directory:
                    raise OpenCodeError("opencode_invalid_response")
                updated = item["time"].get("updated")
                found.append(candidate(valid_session_id(item.get("id")), directory, runtime_status(statuses, item["id"]),
                                       "server_status", "opencode_server", server.url,
                                       int(updated // 1000) if type(updated) is int else None))
            except (ValueError, TypeError, KeyError):
                source.update(status="partial", detail="opencode_invalid_session_records",
                              rejected=source.get("rejected", 0) + 1)
    except (OSError, ValueError, TypeError, KeyError) as exc:
        fixed = isinstance(exc, (OpenCodeError, Uncertain)) or str(exc) in ("invalid_opencode_url", "opencode_url_must_be_loopback")
        source.update(status="unavailable", error=str(exc) if fixed else "opencode_" + type(exc).__name__)
        found = []
    return found, source


def saved_sessions(path):
    """Unarchived root sessions from the local SQLite metadata, read-only. Liveness is unknown."""
    source = {"harness": "opencode", "source": "opencode_saved", "path": str(path), "status": "ok",
              "error": None, "detail": None}
    found = []
    if not path.is_file():
        source.update(status="unavailable", error="opencode_saved_metadata_missing")
        return found, source
    try:
        with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True, timeout=3)) as db:
            rows = db.execute("""SELECT id, directory, time_updated FROM session
                WHERE parent_id IS NULL AND time_archived IS NULL
                ORDER BY time_updated DESC LIMIT ?""", (SAVED_LIMIT + 1,)).fetchall()
        if len(rows) > SAVED_LIMIT:
            rows = rows[:SAVED_LIMIT]
            source.update(status="partial", detail="opencode_saved_session_limit_reached")
        for session_id, directory, updated in rows:
            try:
                if not isinstance(directory, str) or not directory:
                    raise ValueError("opencode_invalid_saved_metadata")
                found.append(candidate(valid_session_id(session_id), directory, "unknown", "saved_metadata_only",
                                       "opencode_saved", None, int(updated // 1000) if type(updated) is int else None))
            except (ValueError, TypeError):
                source.update(status="partial", detail=source["detail"] or "opencode_invalid_saved_metadata",
                              rejected=source.get("rejected", 0) + 1)
    except (sqlite3.Error, OSError, ValueError, TypeError) as exc:
        source.update(status="unavailable", detail=None,
                      error="opencode_saved_" + type(exc).__name__ if isinstance(exc, (sqlite3.Error, OSError)) else str(exc))
        found = []
    return found, source


def discover(urls=None, *, workspace=None, database=None):
    """Read-only discovery: {"sessions": [...], "sources": [...]}. GET requests
    and a read-only metadata file only; no session is created and no turn starts.
    A malformed URL is confined to its own source entry."""
    sessions, sources, seen = [], [], set()
    for url in [DEFAULT_URL] if urls is None else urls:
        found, source = server_sessions(url, workspace)
        sources.append(source)
        for item in found:
            if item["session_id"] not in seen:
                seen.add(item["session_id"])
                sessions.append(item)
    found, source = saved_sessions(Path(database) if database else saved_database())
    sources.append(source)
    sessions.extend(item for item in found if item["session_id"] not in seen)
    return {"sessions": sessions, "sources": sources}
