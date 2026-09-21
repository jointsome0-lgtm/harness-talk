//! One shared database. Reading never acknowledges or sends anything.
use crate::{
    error::{Error, Failure},
    model::*,
    os, validate,
};
use rusqlite::{Connection, OpenFlags, Params, TransactionBehavior, params, types::Value as Sql};
use serde_json::{Map, Value};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{fs, io, thread};

pub const SCHEMA_VERSION: i64 = 2;
/// An answer is skipped only on evidence: the recipient acknowledged it, or the
/// recipient's own wait already returned it. A registered wait is merely a hint
/// that such a receipt may arrive within WAIT_GRACE seconds; it never suppresses.
pub const WAIT_GRACE: f64 = 1.0;
pub const PAGE_LIMIT: i64 = 20;
// Every peer row carries retired_at, which is null for an active peer.
const PEER_ROWS: &str =
    "SELECT p.*, r.retired_at FROM peers p LEFT JOIN retired_peers r ON r.name = p.name";
const INBOX_ACTION: &str =
    "Read and ack messages explicitly. Unanswered questions remain until replied to.";
const INBOX: &str = "m.recipient=? AND
            ((m.in_reply_to IS NULL AND NOT EXISTS (SELECT 1 FROM messages r WHERE r.in_reply_to=m.id)) OR
             (m.in_reply_to IS NOT NULL AND m.ack_at IS NULL))";
const SENT: &str = "m.sender=?";

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    create: bool,
}

fn code(c: &str) -> Error {
    Error::code(c)
}

/// The recorded detail of a failure that is not a typed outcome: htalk's own
/// validation errors are plain ValueErrors, others keep only their class name.
fn failure_detail(e: Error) -> String {
    match e {
        Error::Code(_) => "ValueError".into(),
        Error::Db(e) => Failure::from(e).to_string(),
        Error::Io(e) => Failure::from(e).to_string(),
        Error::Interrupted => "KeyboardInterrupt".into(),
    }
}

fn read_row(r: &rusqlite::Row) -> Result<Row, Error> {
    Ok(Row {
        seq: r.get("seq")?,
        id: r.get("id")?,
        sender: r.get("sender")?,
        recipient: r.get("recipient")?,
        in_reply_to: r.get("in_reply_to")?,
        body: r.get("body")?,
        created_at: r.get("created_at")?,
        ack_at: r.get("ack_at")?,
        submission: r.get::<_, String>("submission")?.parse()?,
        notification_started_at: r.get("notification_started_at")?,
        notification_finished_at: r.get("notification_finished_at")?,
        notification_detail: r.get("notification_detail")?,
        wait_returned_at: r.get("wait_returned_at")?,
    })
}

fn read_peer(r: &rusqlite::Row) -> Result<Peer, Error> {
    Ok(Peer {
        name: r.get("name")?,
        harness: r.get::<_, String>("harness")?.parse()?,
        session_id: r.get("session_id")?,
        workspace: r.get("workspace")?,
        socket: r.get("socket")?,
        url: r.get("url")?,
        retired_at: r.get("retired_at")?,
    })
}

fn first<T>(
    db: &Connection,
    sql: &str,
    p: impl Params,
    read: fn(&rusqlite::Row) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query(p)?;
    match rows.next()? {
        Some(r) => Ok(Some(read(r)?)),
        None => Ok(None),
    }
}

fn message_row(db: &Connection, sql: &str, p: impl Params) -> Result<Option<Row>, Error> {
    first(db, sql, p, read_row)
}

fn with_reply(db: &Connection, row: Row) -> Result<Message, Error> {
    let reply = message_row(db, "SELECT * FROM messages WHERE in_reply_to=?", [&row.id])?;
    Ok(Message::from_row(row, reply))
}

fn load(db: &Connection, id: &str, actor: Option<&str>) -> Result<Message, Error> {
    let row = message_row(db, "SELECT * FROM messages WHERE id=?", [id])?
        .ok_or_else(|| code("unknown_message"))?;
    if let Some(actor) = actor
        && actor != row.sender
        && actor != row.recipient
    {
        return Err(code("message_not_addressed_to_peer"));
    }
    with_reply(db, row)
}

fn is_null(db: &Connection, sql: &str, p: impl Params) -> Result<Option<Vec<bool>>, Error> {
    first(db, sql, p, |r| {
        Ok((0..r.as_ref().column_count())
            .map(|i| r.get::<_, Sql>(i).map(|v| v == Sql::Null))
            .collect::<Result<_, _>>()?)
    })
}

/// Why a claimed notification is no longer needed, or None. Read-only: the
/// recipient acknowledged the message, its own wait already returned this answer,
/// or the recipient peer is retired.
fn skip_reason(
    db: &Connection,
    message_id: &str,
    recipient: &str,
) -> Result<Option<SkipReason>, Error> {
    let row = is_null(
        db,
        "SELECT ack_at, in_reply_to, wait_returned_at FROM messages WHERE id=? AND recipient=?",
        [message_id, recipient],
    )?
    .ok_or_else(|| code("notification_message_not_found"))?;
    if let [ack_null, request_null, returned_null] = row[..] {
        if !ack_null {
            return Ok(Some(SkipReason::AcknowledgedBeforeNotification));
        }
        if !request_null && !returned_null {
            return Ok(Some(SkipReason::ReturnedByRecipientWait));
        }
    }
    if is_null(db, "SELECT 1 FROM retired_peers WHERE name=?", [recipient])?.is_some() {
        return Ok(Some(SkipReason::RecipientRetired));
    }
    Ok(None)
}

/// Latest registered poll by the recipient on this answer's request, or None.
fn wait_deadline(db: &Connection, message_id: &str, recipient: &str) -> Result<Option<f64>, Error> {
    Ok(db.query_row(
        "SELECT MAX(w.until) FROM waits w JOIN messages m ON m.in_reply_to = w.message_id
        WHERE m.id=? AND w.actor=? AND w.actor=m.recipient",
        [message_id, recipient],
        |r| r.get(0),
    )?)
}

/// Replace a message's and its answer's text with its UTF-8 size and first nonblank line.
fn summarize(message: &mut Value) {
    let Some(message) = message.as_object_mut() else {
        return;
    };
    fn one(item: &mut Map<String, Value>) {
        if let Some(Value::String(body)) = item.shift_remove("body") {
            item.insert("body_bytes".into(), body.len().into());
            item.insert("body_preview".into(), validate::preview(&body).into());
        }
    }
    if let Some(Value::Object(reply)) = message.get_mut("reply") {
        one(reply);
    }
    one(message);
}

/// Python's `Path.mkdir(mode, parents=True, exist_ok=True)`: only the last directory gets `mode`.
fn make_dir(path: &Path, mode: u32) -> io::Result<()> {
    match fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                make_dir(parent, 0o777)?;
            }
            match fs::DirBuilder::new().mode(mode).create(path) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
                other => other,
            }
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Removes the wait registration however the wait ends, including interruption.
struct Registration<'a> {
    store: &'a Store,
    token: Option<String>,
}
impl Drop for Registration<'_> {
    fn drop(&mut self) {
        if let Some(token) = &self.token {
            // A leftover row only makes one later reply wait WAIT_GRACE.
            let _ = self
                .store
                .connect()
                .and_then(|db| Ok(db.execute("DELETE FROM waits WHERE token=?", [token])?));
        }
    }
}

impl Store {
    pub fn open(path: &Path, create: bool) -> Result<Self, Error> {
        let store = Store {
            path: os::resolve(path),
            create,
        };
        if create {
            if let Some(parent) = store.path.parent() {
                make_dir(parent, 0o700)?;
            }
            // Create privately before sqlite opens it, independent of the caller's umask.
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&store.path)?;
        } else if let Err(e) = fs::metadata(&store.path) {
            // Access and corruption errors surface separately when opening.
            return Err(
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) {
                    code("database_not_found")
                } else {
                    e.into()
                },
            );
        }
        let mut db = store.connect()?;
        // One write transaction: concurrent first opens serialize here, so the
        // schema check, table creation and column addition cannot interleave.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if !matches!(version, 0 | 1 | SCHEMA_VERSION) {
            return Err(code("unsupported_database_version"));
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS peers (
                    name TEXT PRIMARY KEY, harness TEXT NOT NULL,
                    session_id TEXT NOT NULL, workspace TEXT NOT NULL,
                    socket TEXT, url TEXT, UNIQUE(harness, session_id))",
        )?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL,
                    sender TEXT NOT NULL REFERENCES peers(name),
                    recipient TEXT NOT NULL REFERENCES peers(name),
                    in_reply_to TEXT UNIQUE REFERENCES messages(id),
                    body TEXT NOT NULL, created_at REAL NOT NULL, ack_at REAL,
                    submission TEXT NOT NULL CHECK(submission IN
                        ('not_submitted', 'submission_unknown', 'submitted')),
                    notification_started_at REAL, notification_finished_at REAL,
                    notification_detail TEXT)",
        )?;
        let columns = |table: &str| -> Result<Vec<String>, Error> {
            let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<_, _>>()?;
            Ok(names)
        };
        // Version 2 adds the nullable OpenCode server URL. Rows, marks and
        // addresses are untouched; 0.2 clients reject version 2 explicitly.
        if !columns("peers")?.iter().any(|c| c == "url") {
            tx.execute_batch("ALTER TABLE peers ADD COLUMN url TEXT")?;
        }
        // Additive, version unchanged: a 0.3.0 client ignores both, so its waits
        // and replies behave as before. wait_returned_at records that the
        // recipient's wait returned an answer; waits lists polls in progress.
        if !columns("messages")?.iter().any(|c| c == "wait_returned_at") {
            tx.execute_batch("ALTER TABLE messages ADD COLUMN wait_returned_at REAL")?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS waits (
                    token TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id),
                    actor TEXT NOT NULL, until REAL NOT NULL)",
        )?;
        // Also additive: 0.3 clients ignore retirement, and their peers inserts keep six columns.
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS retired_peers (
                    name TEXT PRIMARY KEY REFERENCES peers(name), retired_at REAL NOT NULL)",
        )?;
        tx.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION}"))?;
        tx.commit()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A fresh connection with htalk's per-connection settings. Without create, a
    /// file removed after opening fails to open instead of reappearing empty.
    pub fn connect(&self) -> Result<Connection, Error> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if self.create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let db = Connection::open_with_flags(&self.path, flags)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON")?;
        Ok(db)
    }

    pub fn add_peer(
        &self,
        name: &str,
        harness: Harness,
        session: &str,
        workspace: &str,
        socket: Option<&str>,
        url: Option<&str>,
    ) -> Result<Peer, Error> {
        validate::peer_name(name)?;
        // OpenCode identifiers are opaque; the other harnesses use UUIDs.
        let session_id = if harness == Harness::Opencode {
            validate::opencode_session_id(session)?
        } else {
            validate::uuid(session)?
        };
        let url = if harness == Harness::Opencode {
            if socket.is_some() {
                return Err(code("opencode_uses_a_server_url_not_a_socket"));
            }
            Some(validate::opencode_url(url)?)
        } else if url.is_some() {
            return Err(code("url_is_only_for_opencode"));
        } else {
            None
        };
        let workspace = os::resolve_strict(Path::new(workspace))?;
        if !workspace.is_dir() {
            return Err(code("workspace_must_be_a_directory"));
        }
        let workspace = workspace.to_string_lossy().into_owned();
        let socket = socket.map(|s| os::resolve(Path::new(s)).to_string_lossy().into_owned());
        if harness == Harness::Claude && socket.is_some() {
            return Err(code("claude_socket_is_discovered_from_live_identity"));
        }
        let text = |v: &str| Sql::Text(v.to_owned());
        let optional = |v: &Option<String>| v.as_deref().map_or(Sql::Null, text);
        let values = vec![
            text(name),
            text(harness.as_str()),
            text(&session_id),
            text(&workspace),
            optional(&socket),
            optional(&url),
        ];
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = first(&tx, "SELECT * FROM peers WHERE name=?", [name], |r| {
            Ok((0..r.as_ref().column_count())
                .map(|i| r.get::<_, Sql>(i))
                .collect::<Result<Vec<_>, _>>()?)
        })?;
        match existing {
            Some(existing) => {
                if existing != values {
                    return Err(code("peer_already_has_a_different_address"));
                }
            }
            None => {
                if first(
                    &tx,
                    "SELECT 1 FROM peers WHERE harness=? AND session_id=?",
                    [harness.as_str(), &session_id],
                    |_| Ok(()),
                )?
                .is_some()
                {
                    return Err(code("session_already_has_a_peer_name"));
                }
                tx.execute(
                    "INSERT INTO peers VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params_from_iter(&values),
                )?;
            }
        }
        tx.commit()?;
        self.peer(name)
    }

    pub fn peer(&self, name: &str) -> Result<Peer, Error> {
        first(
            &self.connect()?,
            &format!("{PEER_ROWS} WHERE p.name=?"),
            [name],
            read_peer,
        )?
        .ok_or_else(|| code("unknown_peer"))
    }

    pub fn session_peer(&self, h: Harness, session: &str) -> Result<Option<Peer>, Error> {
        first(
            &self.connect()?,
            &format!("{PEER_ROWS} WHERE p.harness=? AND p.session_id=?"),
            [h.as_str(), session],
            read_peer,
        )
    }

    /// All registered peers, including retired ones.
    pub fn peers(&self) -> Result<Vec<Peer>, Error> {
        let db = self.connect()?;
        let mut stmt = db.prepare(&format!("{PEER_ROWS} ORDER BY p.name"))?;
        let mut rows = stmt.query([])?;
        let mut peers = Vec::new();
        while let Some(r) = rows.next()? {
            peers.push(read_peer(r)?);
        }
        Ok(peers)
    }

    /// Refuse new requests to or from the peer. Repeating keeps the first time.
    pub fn retire(&self, name: &str) -> Result<Peer, Error> {
        self.peer(name)?;
        self.connect()?.execute(
            "INSERT OR IGNORE INTO retired_peers VALUES (?, ?)",
            params![name, os::now()],
        )?;
        self.peer(name)
    }

    /// Allow new requests again. Notices skipped while retired are not replayed.
    pub fn restore(&self, name: &str) -> Result<Peer, Error> {
        self.peer(name)?;
        self.connect()?
            .execute("DELETE FROM retired_peers WHERE name=?", [name])?;
        self.peer(name)
    }

    pub fn save(
        &self,
        sender: &str,
        recipient: &str,
        body: &str,
        id: Option<&str>,
        in_reply_to: Option<&str>,
    ) -> Result<(Message, bool), Error> {
        validate::text(body)?;
        self.peer(sender)?;
        self.peer(recipient)?;
        if sender == recipient {
            return Err(code("sender_and_recipient_must_differ"));
        }
        let id = match id.filter(|v| !v.is_empty()) {
            Some(v) => validate::uuid(v)?,
            None => uuid::Uuid::new_v4().to_string(),
        };
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(request) = in_reply_to.filter(|v| !v.is_empty()) {
            let parent = message_row(&tx, "SELECT * FROM messages WHERE id=?", [request])?
                .ok_or_else(|| code("unknown_request"))?;
            if parent.in_reply_to.is_some() {
                return Err(code("reply_requires_a_request"));
            }
            if (sender, recipient) != (parent.recipient.as_str(), parent.sender.as_str()) {
                return Err(code("reply_address_mismatch"));
            }
            if let Some(existing) =
                message_row(&tx, "SELECT * FROM messages WHERE in_reply_to=?", [request])?
            {
                if existing.body != body {
                    return Err(code("reply_conflict_existing_answer_preserved"));
                }
                return Ok((with_reply(&tx, existing)?, false));
            }
        }
        if let Some(existing) = message_row(&tx, "SELECT * FROM messages WHERE id=?", [&id])? {
            if (
                existing.sender.as_str(),
                existing.recipient.as_str(),
                existing.body.as_str(),
                existing.in_reply_to.as_deref(),
            ) != (sender, recipient, body, in_reply_to)
            {
                return Err(code("message_id_conflict"));
            }
            return Ok((with_reply(&tx, existing)?, false));
        }
        // Retirement refuses new requests only; saved requests can still be answered.
        if in_reply_to.is_none()
            && first(
                &tx,
                "SELECT 1 FROM retired_peers WHERE name IN (?, ?)",
                [sender, recipient],
                |_| Ok(()),
            )?
            .is_some()
        {
            return Err(code("peer_retired"));
        }
        tx.execute(
            "INSERT INTO messages
                (id, sender, recipient, in_reply_to, body, created_at, submission)
                VALUES (?, ?, ?, ?, ?, ?, 'not_submitted')",
            params![id, sender, recipient, in_reply_to, body, os::now()],
        )?;
        tx.commit()?;
        Ok((self.get(&id, None)?, true))
    }

    pub fn notify_once(
        &self,
        id: &str,
        notify: &dyn Fn(&Peer, &Message) -> Outcome,
        dismiss: &dyn Fn(&Peer, &Message) -> Cleanup,
    ) -> Result<Message, Error> {
        // Claim durably BEFORE crossing the client boundary. A crash stays unknown.
        let claimed = self.connect()?.execute(
            "UPDATE messages SET submission='submission_unknown',
                notification_started_at=? WHERE id=? AND notification_started_at IS NULL
                AND ack_at IS NULL",
            params![os::now(), id],
        )?;
        if claimed == 0 {
            return self.get(id, None);
        }
        let message = self.get(id, None)?;
        let reason = match self.skip_after_grace(&message) {
            Ok(reason) => reason,
            Err(Error::Db(_)) => None, // The adapter repeats this check before writing.
            Err(e) => return Err(e),
        };
        if os::interrupted() {
            return Err(Error::Interrupted);
        }
        let outcome = match reason {
            Some(reason) => Outcome::not_submitted(reason.as_str()),
            None => match self.peer(&message.row.recipient) {
                Ok(peer) => notify(&peer, &message),
                Err(e) => Outcome::unknown(failure_detail(e)),
            },
        };
        // SIGINT leaves the durable claim unfinished, as a KeyboardInterrupt
        // did in 0.4. A caller must inspect it and must never replay the notice.
        if os::interrupted() {
            return Err(Error::Interrupted);
        }
        self.connect()?.execute(
            "UPDATE messages SET submission=?, notification_detail=?,
                notification_finished_at=? WHERE id=?",
            params![outcome.submission.as_str(), outcome.detail, os::now(), id],
        )?;
        // The recipient may ack while the adapter is still submitting. Once the
        // queue receipt is saved, the sender completes the same idempotent cleanup.
        self.dismiss_acknowledged(self.get(id, None)?, dismiss)
    }

    /// Why a notification for this message is no longer needed, read from the store.
    pub fn skip_reason(&self, message: &Message) -> Result<Option<SkipReason>, Error> {
        skip_reason(&self.connect()?, &message.row.id, &message.row.recipient)
    }

    /// While the recipient has a poll registered on this answer's request, give
    /// that poll up to WAIT_GRACE seconds to return the answer and record it. A
    /// stale registration only costs this bounded wait; it never skips by itself.
    fn skip_after_grace(&self, message: &Message) -> Result<Option<SkipReason>, Error> {
        let deadline = wait_deadline(&self.connect()?, &message.row.id, &message.row.recipient)?;
        let grace = deadline.map_or(0.0, |until| (until - os::now()).clamp(0.0, WAIT_GRACE));
        let end = Instant::now() + Duration::from_secs_f64(grace);
        loop {
            let reason = self.skip_reason(message)?;
            if reason.is_some() || Instant::now() >= end || os::interrupted() {
                return Ok(reason);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn dismiss_acknowledged(
        &self,
        mut message: Message,
        dismiss: &dyn Fn(&Peer, &Message) -> Cleanup,
    ) -> Result<Message, Error> {
        if message.row.ack_at.is_some() {
            message.notification_cleanup = Some(match self.peer(&message.row.recipient) {
                Ok(peer) => dismiss(&peer, &message),
                Err(e) => Cleanup::new(CleanupStatus::Unknown).detail(failure_detail(e)),
            });
        }
        Ok(message)
    }

    pub fn get(&self, id: &str, actor: Option<&str>) -> Result<Message, Error> {
        load(&self.connect()?, id, actor)
    }

    pub fn ack(
        &self,
        id: &str,
        actor: &str,
        dismiss: &dyn Fn(&Peer, &Message) -> Cleanup,
    ) -> Result<Message, Error> {
        if self.connect()?.execute(
            "UPDATE messages SET ack_at=COALESCE(ack_at, ?)
                WHERE id=? AND recipient=?",
            params![os::now(), id, actor],
        )? == 0
        {
            return Err(code("only_recipient_can_ack"));
        }
        self.dismiss_acknowledged(self.get(id, Some(actor))?, dismiss)
    }

    pub fn wait(&self, id: &str, actor: &str, seconds: f64) -> Result<Message, Error> {
        validate::wait(seconds)?;
        let request = self.get(id, Some(actor))?;
        if request.row.sender != actor || request.row.in_reply_to.is_some() {
            return Err(code("wait_requires_own_request"));
        }
        // Register the poll so a reply saved meanwhile waits briefly for this
        // command to return it; the return itself is recorded on the answer.
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        let mut registration = Registration {
            store: self,
            token: None,
        };
        if seconds > 0.0 {
            let token = uuid::Uuid::new_v4().to_string();
            let mut db = self.connect()?;
            let tx = db.transaction()?;
            tx.execute("DELETE FROM waits WHERE until < ?", [os::now() - 60.0])?;
            registration.token = Some(token.clone());
            tx.execute(
                "INSERT INTO waits VALUES (?, ?, ?, ?)",
                params![token, id, actor, os::now() + seconds],
            )?;
            tx.commit()?;
        }
        loop {
            let mut result = self.get(id, Some(actor))?;
            if let Some(reply) = &result.reply {
                self.record_wait_return(&reply.id, actor);
                return self.get(id, Some(actor));
            }
            let now = Instant::now();
            if now >= deadline {
                result.wait_ended = Some("timeout".into());
                return Ok(result);
            }
            if os::interrupted() {
                return Err(Error::Interrupted);
            }
            thread::sleep((deadline - now).min(Duration::from_millis(100)));
        }
    }

    /// The recipient's wait is returning this answer: a durable receipt that a
    /// not-yet-sent notice is redundant. Not an acknowledgment.
    fn record_wait_return(&self, answer_id: &str, actor: &str) {
        // On failure the answer is still returned; at worst a redundant notice follows.
        let _ = self.connect().and_then(|db| Ok(db.execute(
            "UPDATE messages SET wait_returned_at=COALESCE(wait_returned_at, ?) WHERE id=? AND recipient=?",
            params![os::now(), answer_id, actor])?));
    }

    /// One page of the actor's matching messages, ordered by seq. Counts and rows
    /// come from one read snapshot. omitted counts matches beyond this page in its
    /// direction; total ignores the cursor and limit.
    fn page(
        &self,
        actor: &str,
        condition: &str,
        newest_first: bool,
        limit: i64,
        cursor: Option<i64>,
    ) -> Result<Page, Error> {
        validate::page(limit, cursor)?;
        self.peer(actor)?;
        let (order, beyond) = if newest_first {
            ("DESC", "<")
        } else {
            ("ASC", ">")
        };
        let cursor = cursor.unwrap_or(if newest_first { i64::MAX } else { 0 });
        let mut db = self.connect()?;
        let tx = db.transaction()?;
        let count = |extra: &str, p: &[&dyn rusqlite::ToSql]| -> Result<i64, Error> {
            Ok(tx.query_row(
                &format!("SELECT COUNT(*) FROM messages m WHERE {condition}{extra}"),
                p,
                |r| r.get(0),
            )?)
        };
        let total = count("", &[&actor])?;
        let mut rows = Vec::new();
        {
            let mut stmt = tx.prepare(&format!("SELECT * FROM messages m WHERE {condition} AND m.seq {beyond} ? ORDER BY m.seq {order} LIMIT ?"))?;
            let mut found = stmt.query(params![actor, cursor, limit])?;
            while let Some(r) = found.next()? {
                rows.push(read_row(r)?);
            }
        }
        let omitted = match rows.last() {
            Some(last) => count(&format!(" AND m.seq {beyond} ?"), &[&actor, &last.seq])?,
            None => 0,
        };
        let messages = rows
            .into_iter()
            .map(|row| Ok(serde_json::to_value(with_reply(&tx, row)?)?))
            .collect::<Result<_, Error>>()?;
        Ok(Page {
            messages,
            total,
            omitted,
            next_action: None,
        })
    }

    /// Unanswered incoming questions and unacknowledged answers, oldest first.
    pub fn inbox(&self, actor: &str, limit: i64, after_seq: Option<i64>) -> Result<Page, Error> {
        let mut page = self.page(actor, INBOX, false, limit, after_seq)?;
        page.next_action = Some(INBOX_ACTION.into());
        Ok(page)
    }

    /// Outgoing messages, newest first. Without bodies, texts are summarized.
    pub fn sent(
        &self,
        actor: &str,
        limit: i64,
        before_seq: Option<i64>,
        bodies: bool,
    ) -> Result<Page, Error> {
        let mut page = self.page(actor, SENT, true, limit, before_seq)?;
        if !bodies {
            page.messages.iter_mut().for_each(summarize);
        }
        Ok(page)
    }
}

/// The transports' final check before a write: a read-only open that never creates
/// or migrates. Only fixed codes cross this boundary.
pub fn skip_reason_readonly(
    db: &Path,
    message_id: &str,
    recipient: &str,
) -> Result<Option<SkipReason>, Failure> {
    let unavailable = || Failure::coded("notification_state_unavailable");
    let conn = Connection::open_with_flags(
        os::resolve(db),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .and_then(|c| c.busy_timeout(Duration::from_secs(3)).map(|()| c))
    .map_err(|_| unavailable())?;
    skip_reason(&conn, message_id, recipient).map_err(|e| match e {
        Error::Code(c) => Failure::Coded(c),
        _ => unavailable(),
    })
}
