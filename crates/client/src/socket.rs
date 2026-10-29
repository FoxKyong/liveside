//! Socket transport: newline-delimited JSON, one wire message per line.
//!
//! Easy to inspect: `socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/liveside/daemon.sock`
//! and type `{"type":"hello","protocol":1,"client":"socat"}`.

use std::io;
use std::path::PathBuf;

use liveside_core::EngineHandle;
use liveside_proto::{ClientMessage, ServerMessage};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::QUEUE;

/// Longest accepted line. Snapshots of many busy channels are the largest
/// messages, so this is generous.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Where the daemon listens by default: `$XDG_RUNTIME_DIR/liveside/daemon.sock`,
/// or a per-user directory under the system temp dir if that is unset.
pub fn default_socket_path() -> PathBuf {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime) if !runtime.is_empty() => PathBuf::from(runtime).join("liveside"),
        _ => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("liveside-{user}"))
        }
    };
    dir.join("daemon.sock")
}

/// Serves one frontend connected over `stream` until it disconnects or the
/// engine stops.
pub async fn serve<S>(engine: EngineHandle, server: String, stream: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, write) = tokio::io::split(stream);
    let (incoming_tx, incoming) = mpsc::channel(QUEUE);
    let (outgoing, outgoing_rx) = mpsc::channel(QUEUE);
    // The daemon owns its stderr, so it may report transport problems.
    let reader = tokio::spawn(read_frames(read, incoming_tx, true));
    let writer = tokio::spawn(write_frames(write, outgoing_rx, true));
    liveside_core::session::run(engine, server, incoming, outgoing).await;
    // The session dropped its sender, so the writer flushes what is queued
    // and ends; the reader may be blocked on a silent peer, so stop it.
    let _ = writer.await;
    reader.abort();
}

/// Frontend side: turns a stream into a message sender and receiver.
pub(crate) fn client<S>(stream: S) -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, write) = tokio::io::split(stream);
    let (to_engine, to_engine_rx) = mpsc::channel(QUEUE);
    let (messages_tx, messages) = mpsc::channel(QUEUE);
    // A frontend may be drawing on this terminal: stay quiet and let the
    // closed connection speak for itself.
    tokio::spawn(read_frames(read, messages_tx, false));
    tokio::spawn(write_frames(write, to_engine_rx, false));
    (to_engine, messages)
}

/// Reads JSON lines into `tx` until EOF, an error, or the receiver is gone.
async fn read_frames<R, T>(read: R, tx: mpsc::Sender<T>, report: bool)
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let limit = (MAX_FRAME + 1) as u64;
        match (&mut reader).take(limit).read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                if report {
                    eprintln!("socket read error: {e}");
                }
                return;
            }
        }
        if buf.last() != Some(&b'\n') {
            if report && buf.len() > MAX_FRAME {
                eprintln!("socket frame longer than {MAX_FRAME} bytes, closing");
            }
            // Otherwise: EOF in the middle of a line, the peer went away.
            return;
        }
        let line = buf.trim_ascii();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(message) => {
                if tx.send(message).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                if report {
                    eprintln!("invalid message on socket, closing: {e}");
                }
                return;
            }
        }
    }
}

/// Writes messages from `rx` as JSON lines until the channel closes or the
/// peer goes away.
async fn write_frames<W, T>(mut write: W, mut rx: mpsc::Receiver<T>, report: bool)
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    while let Some(message) = rx.recv().await {
        let mut line = match serde_json::to_vec(&message) {
            Ok(line) => line,
            Err(e) => {
                if report {
                    eprintln!("cannot serialize message: {e}");
                }
                continue;
            }
        };
        line.push(b'\n');
        if let Err(e) = write.write_all(&line).await {
            if report && e.kind() != io::ErrorKind::BrokenPipe {
                eprintln!("socket write error: {e}");
            }
            return;
        }
    }
    let _ = write.shutdown().await;
}
