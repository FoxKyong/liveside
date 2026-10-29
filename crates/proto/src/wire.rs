//! Envelopes exchanged between a frontend and the engine.
//!
//! In-process mode passes these values over channels, daemon mode sends them
//! serialized over a socket. The session is:
//!
//! 1. frontend → `Hello`, engine → `Welcome` (or closes on version mismatch),
//! 2. engine → `Snapshot`,
//! 3. then any number of `Request`s one way and `Event`s / `Response`s the
//!    other way.

use serde::{Deserialize, Serialize};

use crate::command::{Command, CommandError, CommandResult};
use crate::event::{Event, Snapshot};
use crate::ids::{RequestId, Seq};

/// Version of this protocol. Bumped on every incompatible change; engine and
/// frontend must agree exactly.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol: u32,
        /// Free-form frontend name and version, for logs.
        client: String,
    },
    Request {
        id: RequestId,
        #[serde(flatten)]
        command: Command,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol: u32,
        /// Free-form engine name and version, for logs.
        server: String,
    },
    /// Refusal before closing, e.g. protocol version mismatch.
    Rejected { reason: String },
    /// Full state as of `seq`; the next event has a greater sequence number.
    Snapshot {
        seq: Seq,
        #[serde(flatten)]
        snapshot: Snapshot,
    },
    Event {
        seq: Seq,
        #[serde(flatten)]
        event: Event,
    },
    /// Answer to the frontend's own request, sent after every event the
    /// request caused. Not sent to other frontends. No `error` means success.
    Response {
        id: RequestId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<CommandError>,
    },
}

impl ServerMessage {
    pub fn response(id: RequestId, result: CommandResult) -> Self {
        ServerMessage::Response {
            id,
            error: result.err(),
        }
    }
}
