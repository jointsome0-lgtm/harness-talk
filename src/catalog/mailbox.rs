//! The catalogue's own read-only view of a mailbox. These reads cannot create, initialize
//! or migrate one.
use crate::{
    error::Error,
    model::{Message, Page, Peer},
    store::{INBOX, SCHEMA_VERSION, SENT, load, peer_on, read_row, summarize, with_reply},
    validate,
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{path::Path, time::Duration};

fn connection(path: &Path) -> Result<Connection, Error> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(2))?;
    if db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? != SCHEMA_VERSION {
        return Err(Error::code("catalog_requires_schema3"));
    }
    Ok(db)
}

pub(super) fn peers(path: &Path, names: &[String]) -> Result<Vec<Peer>, Error> {
    let db = connection(path)?;
    db.execute_batch("BEGIN")?;
    names.iter().map(|name| peer_on(&db, name)).collect()
}

pub(super) fn message(path: &Path, id: &str, actor: &str) -> Result<Message, Error> {
    let db = connection(path)?;
    db.execute_batch("BEGIN")?;
    load(&db, &validate::uuid(id)?, Some(actor))
}

/// Apply the catalogue's conversation filter before pagination and counts.
pub(super) fn page(
    path: &Path,
    actor: &str,
    names: &[String],
    sent: bool,
    limit: i64,
    cursor: Option<i64>,
    bodies: bool,
) -> Result<Page, Error> {
    validate::page(limit, cursor)?;
    let mut db = connection(path)?;
    let tx = db.transaction()?;
    peer_on(&tx, actor)?;
    let base = if sent { SENT } else { INBOX };
    let condition = format!(
        "({base}) AND CASE WHEN m.sender=?1 THEN m.recipient ELSE m.sender END IN (SELECT value FROM json_each(?2))"
    );
    let (order, beyond) = if sent { ("DESC", "<") } else { ("ASC", ">") };
    let cursor = cursor.unwrap_or(if sent { i64::MAX } else { 0 });
    let names = serde_json::to_string(names)?;
    let total = tx.query_row(
        &format!("SELECT COUNT(*) FROM messages m WHERE {condition}"),
        params![actor, names],
        |r| r.get(0),
    )?;
    let mut stmt = tx.prepare(&format!("SELECT * FROM messages m WHERE {condition} AND m.seq {beyond} ?3 ORDER BY m.seq {order} LIMIT ?4"))?;
    let mut found = stmt.query(params![actor, names, cursor, limit])?;
    let mut rows = Vec::new();
    while let Some(r) = found.next()? {
        rows.push(read_row(r)?);
    }
    let omitted = match rows.last() {
        Some(last) => tx.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {condition} AND m.seq {beyond} ?3"),
            params![actor, names, last.seq],
            |r| r.get(0),
        )?,
        None => 0,
    };
    let mut messages: Vec<Value> = rows
        .into_iter()
        .map(|r| Ok(serde_json::to_value(with_reply(&tx, r)?)?))
        .collect::<Result<_, Error>>()?;
    if sent && !bodies {
        messages.iter_mut().for_each(summarize);
    }
    Ok(Page {
        messages,
        total,
        omitted,
    })
}
