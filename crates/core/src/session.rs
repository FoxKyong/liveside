//! One frontend's session with the engine, in terms of wire messages.
//!
//! Transport-agnostic: the caller connects `incoming`/`outgoing` to in-process
//! channels or to a socket. Implements the sequence from
//! `liveside_proto::wire`: hello → welcome → snapshot → requests and events.
//!
//! Ordering guarantee: a request's response is sent after every event the
//! request caused.

use liveside_proto::{ClientMessage, PROTOCOL_VERSION, RequestId, Seq, ServerMessage};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;

use crate::engine::{Completion, EngineHandle, Sequenced};

/// Runs a session until the frontend goes away or the engine stops.
///
/// `server` is the engine's name and version, sent in the welcome.
pub async fn run(
    engine: EngineHandle,
    server: String,
    mut incoming: mpsc::Receiver<ClientMessage>,
    outgoing: mpsc::Sender<ServerMessage>,
) {
    match incoming.recv().await {
        Some(ClientMessage::Hello { protocol, .. }) if protocol == PROTOCOL_VERSION => {}
        Some(ClientMessage::Hello { protocol, .. }) => {
            let reason = format!(
                "Protocol version mismatch: frontend {protocol}, engine {PROTOCOL_VERSION}"
            );
            let _ = outgoing.send(ServerMessage::Rejected { reason }).await;
            return;
        }
        Some(ClientMessage::Request { .. }) => {
            let reason = "Expected hello as the first message".to_owned();
            let _ = outgoing.send(ServerMessage::Rejected { reason }).await;
            return;
        }
        None => return,
    }
    let welcome = ServerMessage::Welcome {
        protocol: PROTOCOL_VERSION,
        server,
    };
    if outgoing.send(welcome).await.is_err() {
        return;
    }

    let Some(mut stream) = EventStream::attach(&engine, &outgoing).await else {
        return;
    };
    // Completed commands, collected from per-request tasks.
    let (done_tx, mut done) = mpsc::unbounded_channel::<(RequestId, Completion)>();

    loop {
        tokio::select! {
            message = incoming.recv() => match message {
                Some(ClientMessage::Request { id, command }) => {
                    let Ok(completion) = engine.command(command).await else {
                        return;
                    };
                    let done_tx = done_tx.clone();
                    tokio::spawn(async move {
                        if let Ok(completion) = completion.await {
                            let _ = done_tx.send((id, completion));
                        }
                    });
                }
                // A repeated hello is harmless; ignore it.
                Some(ClientMessage::Hello { .. }) => {}
                None => return,
            },
            Some((id, completion)) = done.recv() => {
                // Deliver the command's effects first.
                while stream.last < completion.seq {
                    if stream.forward_next(&engine, &outgoing).await.is_none() {
                        return;
                    }
                }
                let response = ServerMessage::response(id, completion.result);
                if outgoing.send(response).await.is_err() {
                    return;
                }
            }
            // Only the receive is raced; sending happens in the handler, so
            // a received event is never dropped by another branch winning.
            received = stream.events.recv() => {
                if stream.handle(received, &engine, &outgoing).await.is_none() {
                    return;
                }
            }
        }
    }
}

/// The engine's event stream as seen by this frontend.
struct EventStream {
    events: broadcast::Receiver<Sequenced>,
    /// Sequence number of the last snapshot or event sent to the frontend.
    last: Seq,
}

impl EventStream {
    /// Attaches to the engine and sends the snapshot. `None` if the engine or
    /// the frontend is gone.
    async fn attach(engine: &EngineHandle, outgoing: &mpsc::Sender<ServerMessage>) -> Option<Self> {
        let attachment = engine.attach().await.ok()?;
        let message = ServerMessage::Snapshot {
            seq: attachment.seq,
            snapshot: attachment.snapshot,
        };
        outgoing.send(message).await.ok()?;
        Some(Self {
            events: attachment.events,
            last: attachment.seq,
        })
    }

    /// Waits for the next event and sends it. Not cancel-safe.
    async fn forward_next(
        &mut self,
        engine: &EngineHandle,
        outgoing: &mpsc::Sender<ServerMessage>,
    ) -> Option<()> {
        let received = self.events.recv().await;
        self.handle(received, engine, outgoing).await
    }

    /// Sends a received event; a frontend that fell too far behind gets a
    /// fresh snapshot instead. `None` if the engine or the frontend is gone.
    async fn handle(
        &mut self,
        received: Result<Sequenced, RecvError>,
        engine: &EngineHandle,
        outgoing: &mpsc::Sender<ServerMessage>,
    ) -> Option<()> {
        match received {
            Ok(sequenced) => {
                self.last = sequenced.seq;
                let message = ServerMessage::Event {
                    seq: sequenced.seq,
                    event: sequenced.event,
                };
                outgoing.send(message).await.ok()
            }
            Err(RecvError::Lagged(_)) => {
                *self = Self::attach(engine, outgoing).await?;
                Some(())
            }
            Err(RecvError::Closed) => None,
        }
    }
}
