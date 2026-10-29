//! The chat engine. Owns all state (accounts, channels, scrollback, settings,
//! highlight rules), executes commands and emits events. Contains no UI code.
//!
//! - [`start`] runs the engine with a backend; [`EngineHandle`] talks to it.
//! - [`session`] speaks the frontend protocol (`liveside-proto` wire messages) on
//!   top of a handle; `liveside-client` connects it to channels or a socket.
//! - [`backend`] is the interface to Twitch; [`demo`] is a fake backend.
//! - [`history`] is the local chat log, which is the only source of the
//!   history restored when joining a channel.

pub mod addressbook;
pub mod backend;
pub mod bots;
pub mod commands;
pub mod config;
pub mod demo;
mod engine;
pub mod history;
pub mod host;
pub mod hotkeys;
pub mod input;
pub mod links;
pub mod open_channels;
pub mod redirects;
pub mod repeated;
pub mod rules;
pub mod session;
pub mod twitch;
pub mod virustotal;

pub use engine::{
    Attachment, Completion, EngineConfig, EngineHandle, EngineStopped, Sequenced, start,
};
