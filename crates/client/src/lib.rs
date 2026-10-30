//! logbook's client, a library: what the CLI and the desktop app share.
//! The audio taken (audio.rs, aec.rs), the session on logbook-server
//! (remote.rs) or in this process (local.rs), the meeting from its first
//! sample to its page (meeting.rs): kept until published (pending.rs) and
//! finished later if it could not be then (finishing.rs),
//! written as Markdown (transcript.rs) into brainy (brainy.rs), its
//! speakers named from voices known on this computer (voices.rs), and,
//! for the app, kept once published (history.rs), with the screenshots
//! in its notes (shots.rs). Messages for the person go through
//! `tracing` at info and warn: the CLI prints them, the app shows them.
//!
//! The `local` feature (default) is the backend in this process, with the
//! engine's models; without it the client streams to a server only, and
//! links no model code (the Android app).

pub mod aec;
pub mod audio;
pub mod brainy;
pub mod finishing;
pub mod history;
#[cfg(feature = "local")]
pub mod local;
pub mod meeting;
pub mod pending;
pub mod remote;
pub mod series;
pub mod shots;
pub mod transcript;
pub mod voices;
