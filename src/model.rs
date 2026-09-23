use crate::error::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Codex,
    Claude,
    Opencode,
}
impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
        }
    }
}
impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl std::str::FromStr for Harness {
    type Err = crate::error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            "opencode" => Ok(Self::Opencode),
            _ => Err(crate::error::Error::code("unsupported_harness")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Submission {
    NotSubmitted,
    SubmissionUnknown,
    Submitted,
}
impl Submission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotSubmitted => "not_submitted",
            Self::SubmissionUnknown => "submission_unknown",
            Self::Submitted => "submitted",
        }
    }
}
impl std::str::FromStr for Submission {
    type Err = crate::error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "not_submitted" => Ok(Self::NotSubmitted),
            "submission_unknown" => Ok(Self::SubmissionUnknown),
            "submitted" => Ok(Self::Submitted),
            _ => Err(crate::error::Error::code("invalid_notification_result")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    PullOnly,
    AcknowledgedBeforeNotification,
    ReturnedByRecipientWait,
    RecipientRetired,
}
impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PullOnly => "pull_only",
            Self::AcknowledgedBeforeNotification => "acknowledged_before_notification",
            Self::ReturnedByRecipientWait => "returned_by_recipient_wait",
            Self::RecipientRetired => "recipient_retired",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pull_only" => Some(Self::PullOnly),
            "acknowledged_before_notification" => Some(Self::AcknowledgedBeforeNotification),
            "returned_by_recipient_wait" => Some(Self::ReturnedByRecipientWait),
            "recipient_retired" => Some(Self::RecipientRetired),
            _ => None,
        }
    }
}
pub type Skip<'a> = &'a dyn Fn() -> Result<Option<SkipReason>, Failure>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Saved,
    ReplyReceived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    Skipped,
    Unsupported,
    Pending,
    Removed,
    Absent,
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub name: String,
    pub harness: String,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
    pub socket: Option<String>,
    pub url: Option<String>,
    pub retired_at: Option<f64>,
    // Existing native peer JSON keeps its original keys.
    #[serde(default, skip_serializing_if = "Delivery::is_native")]
    pub delivery: Delivery,
}
impl Peer {
    pub fn pull(name: &str, harness: &str) -> Self {
        Self {
            name: name.into(),
            harness: harness.into(),
            delivery: Delivery::Pull,
            session_id: None,
            workspace: None,
            socket: None,
            url: None,
            retired_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    #[default]
    Native,
    Pull,
}
impl Delivery {
    pub fn is_native(&self) -> bool {
        *self == Self::Native
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Pull => "pull",
        }
    }
}
impl std::str::FromStr for Delivery {
    type Err = crate::error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "native" => Ok(Self::Native),
            "pull" => Ok(Self::Pull),
            _ => Err(crate::error::Error::code("unsupported_delivery")),
        }
    }
}

/// Validated address passed to the built-in native notification adapters.
#[derive(Debug, Clone, PartialEq)]
pub struct NativePeer {
    pub name: String,
    pub harness: Harness,
    pub session_id: String,
    pub workspace: String,
    pub socket: Option<String>,
    pub url: Option<String>,
    pub retired_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub seq: i64,
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub in_reply_to: Option<String>,
    pub body: String,
    pub created_at: f64,
    pub ack_at: Option<f64>,
    pub submission: Submission,
    pub notification_started_at: Option<f64>,
    pub notification_finished_at: Option<f64>,
    pub notification_detail: Option<String>,
    pub wait_returned_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(flatten)]
    pub row: Row,
    pub reply: Option<Row>,
    pub state: MessageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_cleanup: Option<Cleanup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_ended: Option<String>,
}
impl Message {
    pub fn from_row(row: Row, reply: Option<Row>) -> Self {
        let state = if reply.is_some() {
            MessageState::ReplyReceived
        } else {
            MessageState::Saved
        };
        Self {
            row,
            reply,
            state,
            notification_cleanup: None,
            wait_ended: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cleanup {
    pub status: CleanupStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<String>,
}
impl Cleanup {
    pub fn new(status: CleanupStatus) -> Self {
        Self {
            status,
            detail: None,
            queue_id: None,
        }
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn queue_id(mut self, id: impl Into<String>) -> Self {
        self.queue_id = Some(id.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub submission: Submission,
    pub detail: String,
}
impl Outcome {
    pub fn submitted(detail: impl Into<String>) -> Self {
        Self {
            submission: Submission::Submitted,
            detail: detail.into(),
        }
    }
    pub fn not_submitted(detail: impl Into<String>) -> Self {
        Self {
            submission: Submission::NotSubmitted,
            detail: detail.into(),
        }
    }
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            submission: Submission::SubmissionUnknown,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub messages: Vec<Value>,
    pub total: i64,
    pub omitted: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Found {
    pub sessions: Vec<Value>,
    pub sources: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeSession {
    Recognized {
        session_id: String,
        workspace: Option<String>,
    },
    Unrecognized {
        reason: &'static str,
    },
}
impl NativeSession {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Recognized {
                session_id,
                workspace,
            } => {
                json!({"harness":"claude", "status":"recognized", "session_id":session_id, "workspace":workspace})
            }
            Self::Unrecognized { reason } => {
                json!({"harness":"claude", "status":"unrecognized", "reason":reason})
            }
        }
    }
}
