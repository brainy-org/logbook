//! Logbook's engine: libparakeet, the per-channel pipeline, a meeting's
//! session and the threads it shares with the others (batcher, finalizer,
//! polisher), the wire protocol, and the meeting's report written by a
//! language model. logbook-server drives sessions over a WebSocket, the CLI in its own process (`--server` not given); neither
//! holds pipeline logic of its own.
//!
//! The `models` feature (default) is everything that loads or fetches the
//! models: libparakeet, the mirror, the pipeline, the session. Without it
//! only what a client that streams to a server needs is built: the
//! protocol, the Opus codec, recordings, the language model's clean-up and
//! report (plain HTTP). The Android app is built so: it links no model code.

#[cfg(feature = "models")]
pub mod batcher;
#[cfg(feature = "models")]
pub mod channel;
pub mod codec;
pub mod echo;
#[cfg(feature = "models")]
pub mod final_pass;
#[cfg(feature = "models")]
pub mod finalizer;
pub mod finals;
#[cfg(feature = "models")]
pub mod mirror;
#[cfg(feature = "models")]
pub mod parakeet;
pub mod pins;
pub mod polish;
#[cfg(feature = "models")]
pub mod polisher;
#[cfg(feature = "models")]
pub mod progressive;
pub mod protocol;
pub mod recording;
pub mod report;
#[cfg(feature = "models")]
pub mod session;
#[cfg(feature = "models")]
pub mod speakers;
pub mod store;

#[cfg(feature = "models")]
mod engine;
#[cfg(feature = "models")]
pub use engine::*;
