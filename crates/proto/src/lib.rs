//! Protocol between the engine and its frontends.
//!
//! Frontends never call into the engine directly. They send [`Command`]s and
//! receive [`Event`]s, both plain serializable data, so the same frontend code
//! works whether the engine runs in-process or in `liveside-daemon` behind a socket.
//! See `docs/architecture.md`.

pub mod command;
pub mod event;
pub mod ids;
pub mod model;
pub mod wire;

pub use command::{Command, CommandError, CommandResult, ErrorKind, PredictionEnd, join_targets};
pub use event::{
    Account, ChannelState, ConnectionState, DataFlow, DataFlowKind, Event, Snapshot, WhisperThread,
};
pub use ids::{Login, MessageId, RequestId, Rgb, Seq, Timestamp, UserId};
pub use wire::{ClientMessage, PROTOCOL_VERSION, ServerMessage};
