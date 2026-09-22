use crate::os;
use serde_json::{Value, json};
use std::path::Path;

pub fn command(db: &Path, actor: Option<&str>, parts: &[&str]) -> String {
    let resolved = os::resolve(db).to_string_lossy().into_owned();
    let mut words = vec!["htalk", "--db", &resolved];
    if let Some(actor) = actor.filter(|a| !a.is_empty()) {
        words.extend(["--as", actor]);
    }
    words.extend_from_slice(parts);
    os::shell_join(&words)
}

pub fn message_actions(db: &Path, actor: &str, message: &mut Value) {
    let id = message["id"].as_str().unwrap_or("");
    let mut recovery = json!({"show":command(db, Some(actor), &["show", id])});
    let mut action;
    if message["sender"] == actor && message["in_reply_to"].is_null() {
        if !message["reply"].is_null() {
            let answer = &message["reply"];
            let answer_id = answer["id"].as_str().unwrap_or("");
            recovery["show_reply"] = command(db, Some(actor), &["show", answer_id]).into();
            action = "Read the saved answer with recovery.show_reply.".to_owned();
            if answer["ack_at"].is_null() {
                recovery["ack_after_reading"] =
                    command(db, Some(actor), &["ack", answer_id]).into();
                action.push_str(" After reading, use recovery.ack_after_reading.");
            }
        } else {
            recovery["wait"] = command(db, Some(actor), &["wait", id, "--seconds", "45"]).into();
            action = "Use recovery.wait to wait again on this saved request, or recovery.show to inspect it.".to_owned();
        }
    } else if message["recipient"] == actor {
        action = "Read the message with recovery.show.".to_owned();
        if message["ack_at"].is_null() {
            recovery["ack_after_reading"] = command(db, Some(actor), &["ack", id]).into();
            action.push_str(" After reading, use recovery.ack_after_reading.");
        }
        if message["in_reply_to"].is_null() && message["reply"].is_null() {
            action.push_str(" Acknowledging a question leaves it open until you reply.");
        }
    } else {
        action = "Inspect the saved answer with recovery.show; the recipient can retrieve it from their inbox.".to_owned();
        if message["notification_detail"] == "returned_by_recipient_wait" {
            action.push_str(" The recipient's wait recorded this answer for return, so no client notice was sent.");
        }
    }
    if message["notification_detail"] == "recipient_retired" {
        action.push_str(" The recipient peer is retired, so no client notice was sent.");
    }
    let cleanup = message["notification_cleanup"]["status"].as_str();
    if message["recipient"] == actor
        && matches!(cleanup, Some("pending" | "unknown" | "unavailable"))
    {
        recovery["retry_notification_cleanup"] = command(db, Some(actor), &["ack", id]).into();
        action.push_str(" Acknowledgment is saved. After submission finishes, use recovery.retry_notification_cleanup to retry removal.");
        if cleanup == Some("pending") {
            action.push_str(" If the sending process stopped before saving its completion receipt, cleanup can remain pending indefinitely; htalk cannot reconstruct the missing receipt.");
        }
        if cleanup == Some("unavailable") {
            action.push_str(" Verify the registered Codex address and native access. If sandbox restrictions prevented cleanup, use your client's normal approval flow before retrying.");
        }
    }
    action.push_str(" Never repeat an uncertain notification.");
    message["recovery"] = recovery;
    message["next_action"] = action.into();
}

pub fn page_actions(
    db: &Path,
    actor: &str,
    result: &mut Value,
    sent: bool,
    limit: i64,
    bodies: bool,
) {
    if result["omitted"].as_i64().unwrap_or(0) == 0 {
        return;
    }
    let Some(last) = result["messages"]
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|r| r["seq"].as_i64())
    else {
        return;
    };
    let last = last.to_string();
    let limit = limit.to_string();
    let mut parts = if sent {
        vec!["sent", "--limit", &limit, "--before-seq", &last]
    } else {
        vec!["inbox", "--limit", &limit, "--after-seq", &last]
    };
    if sent && bodies {
        parts.push("--bodies");
    }
    result["recovery"] = json!({"next_page":command(db,Some(actor),&parts)});
    let more = if sent { "Older" } else { "Newer" };
    let extra = format!("{more} messages remain: use recovery.next_page.");
    result["next_action"] = match result["next_action"].as_str().filter(|s| !s.is_empty()) {
        Some(previous) => format!("{previous} {extra}"),
        None => extra,
    }
    .into();
}
