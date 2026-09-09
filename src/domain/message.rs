use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One message of a Session transcript, carrying the message's own text.
///
/// Unlike `Record`, which aggregates a whole Session, this is the unit search
/// operates on. `role` is the raw value the Source recorded, consistent with
/// how `model` is kept as a recorded rather than resolved value. Reasoning and
/// thinking blocks are stored as messages with `role: "thinking"` rather than as
/// a separate field, so downstream code never needs a source-specific notion of
/// model-internal text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub source: String,
    pub session_id: String,
    pub project: String,
    pub model: Option<String>,
    pub role: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub text: String,
}
