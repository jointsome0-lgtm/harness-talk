//! `send`, `reply` and `wait`: save a message, notify its recipient once, wait for the answer.
use super::Body;
use crate::{
    cli::{Answer, Context},
    error::Error,
    model::SkipReason,
    notify, os, validate, write_turn,
};
use std::fs::File;

/// Where a message goes: to a peer as a new request, or back as the answer to a request.
pub(crate) enum To<'a> {
    Peer {
        recipient: &'a str,
        id: Option<&'a str>,
    },
    Request(&'a str),
}

/// What a second attempt takes over from the first: the text, read once, and the turn it waited for.
#[derive(Default)]
struct Kept {
    text: Option<String>,
    turn: Option<File>,
}

pub(crate) fn write(
    context: &mut Context,
    to: To,
    body: &Body,
    no_notify: bool,
    wait: f64,
) -> Answer {
    let given = context.actor.clone();
    let mut kept = Kept::default();
    let _probe = write_turn::probe();
    let first = attempt(context, &to, body, no_notify, wait, &mut kept);
    let busy_before_write = matches!(&first, Err(Error::Db(rusqlite::Error::SqliteFailure(e, _)))
        if e.code == rusqlite::ErrorCode::DatabaseBusy)
        && write_turn::fresh();
    if !busy_before_write || os::interrupted() {
        return first;
    }
    // The first attempt ended before any write transaction began. All its
    // read connections and snapshots are gone before waiting for the turn.
    *context = Context::new(context.db.clone(), given);
    kept.turn = write_turn::acquire(&context.db)?;
    write_turn::queued(kept.turn.is_some());
    if os::interrupted() {
        return Err(Error::Interrupted);
    }
    attempt(context, &to, body, no_notify, wait, &mut kept)
}

fn attempt(
    context: &mut Context,
    to: &To,
    body: &Body,
    no_notify: bool,
    wait: f64,
    kept: &mut Kept,
) -> Answer {
    validate::wait(wait)?;
    let session = context.session()?;
    if kept.text.is_none() {
        kept.text = Some(match &body.message_file {
            Some(path) => std::fs::read_to_string(path)?
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
            None => body.message.clone().unwrap_or_default(),
        });
    }
    let text = kept.text.as_deref().unwrap_or_default();
    let (recipient, reply_to, id) = match *to {
        To::Request(request_id) => {
            let request = session.store.get(request_id, Some(session.actor))?;
            (request.row.sender, Some(request_id), None)
        }
        To::Peer { recipient, id } => (recipient.to_owned(), None, id),
    };
    let (mut message, created) =
        session
            .store
            .save(session.actor, &recipient, text, id, reply_to)?;
    write_turn::started_write();
    drop(kept.turn.take());
    *session.saved_id = Some(message.row.id.clone());
    if os::interrupted() {
        return Err(Error::Interrupted);
    }
    let notifies = created && !no_notify;
    if notifies {
        let database = session.store.path();
        message = session.store.notify_once(
            &message.row.id,
            &|p, m| notify::notify(p, m, database),
            &notify::dismiss,
        )?;
    }
    if os::interrupted() {
        return Err(Error::Interrupted);
    }
    if wait != 0.0 {
        message = session.store.wait(&message.row.id, session.actor, wait)?;
    }
    let mut value = serde_json::to_value(message)?;
    value["created"] = created.into();
    let value = session.message(value);
    let unconfirmed = notifies
        && value["reply"].is_null()
        && value["submission"] != "submitted"
        && value["ack_at"].is_null()
        && value["notification_detail"]
            .as_str()
            .and_then(SkipReason::parse)
            .is_none();
    Ok((value, if unconfirmed { 2 } else { 0 }))
}

pub(crate) fn wait(context: &mut Context, message_id: &str, seconds: f64) -> Answer {
    validate::wait(seconds)?;
    let session = context.session()?;
    let message = session.store.wait(message_id, session.actor, seconds)?;
    Ok((session.message(serde_json::to_value(message)?), 0))
}
