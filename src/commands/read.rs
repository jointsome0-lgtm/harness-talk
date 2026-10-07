//! `show`, `ack`, `inbox`, `sent` and `watch`: read saved messages and mark them read.
use crate::{
    cli::{Answer, Session, output},
    error::Error,
    model::Message,
    notify, os, validate,
};
use serde_json::json;
use std::{io, os::fd::AsRawFd, time::Duration};

pub(crate) fn show(session: Session, message_id: &str) -> Answer {
    let message = session.store.get(message_id, Some(session.actor))?;
    Ok((session.message(serde_json::to_value(message)?), 0))
}

pub(crate) fn ack(session: Session, message_id: &str) -> Answer {
    let message = session
        .store
        .ack(message_id, session.actor, &notify::dismiss)?;
    Ok((session.message(serde_json::to_value(message)?), 0))
}

pub(crate) fn inbox(session: Session, limit: &str, after_seq: Option<&str>) -> Answer {
    let (limit, cursor) = page_parameters(limit, after_seq)?;
    let page = session.store.inbox(session.actor, limit, cursor)?;
    Ok((session.page(page, false, limit, false)?, 0))
}

pub(crate) fn sent(
    session: Session,
    limit: &str,
    before_seq: Option<&str>,
    bodies: bool,
) -> Answer {
    let (limit, cursor) = page_parameters(limit, before_seq)?;
    let page = session.store.sent(session.actor, limit, cursor, bodies)?;
    Ok((session.page(page, true, limit, bodies)?, 0))
}

fn page_parameters(limit: &str, cursor: Option<&str>) -> Result<(i64, Option<i64>), Error> {
    let limit = limit
        .parse::<i64>()
        .map_err(|_| Error::code("limit_must_be_between_1_and_500"))?;
    let cursor = cursor
        .map(|v| {
            v.parse::<i64>()
                .map_err(|_| Error::code("seq_cursor_must_be_a_positive_integer"))
        })
        .transpose()?;
    validate::page(limit, cursor)?;
    Ok((limit, cursor))
}

pub(crate) fn watch(session: Session) -> Answer {
    let mut after_seq = None;
    output(&json!({"event":"ready", "peer":session.own.name}))?;
    loop {
        if os::interrupted() {
            return Err(Error::Interrupted);
        }
        // An unloaded/crashed extension closes its read end, even if no new
        // mail arrives. Do not leave an idle watcher behind in that case.
        let mut sink = libc::pollfd {
            fd: io::stdout().as_raw_fd(),
            events: 0,
            revents: 0,
        };
        if unsafe { libc::poll(&mut sink, 1, 0) } > 0
            && sink.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
        {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe).into());
        }
        let page = match session.store.inbox(&session.own.name, 100, after_seq) {
            Ok(page) => page,
            Err(Error::Db(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Err(error) => return Err(error),
        };
        for value in page.messages {
            let message: Message = serde_json::from_value(value)?;
            output(&json!({"event":"message", "id":message.row.id,
                "seq":message.row.seq, "notification":notify::notification(session.own, &message, session.db)}))?;
            after_seq = Some(message.row.seq);
        }
        if page.omitted == 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
