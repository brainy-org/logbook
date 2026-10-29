//! logbook's client, a library: what the CLI and the desktop app share.
//! The audio taken (audio.rs, aec.rs), the session on logbook-server
//! (remote.rs) or in this process (local.rs), the meeting from its first
//! sample to its page (meeting.rs): kept until published (pending.rs),
//! written as Markdown (transcript.rs) into brainy (brainy.rs), its
//! speakers named from voices known on this computer (voices.rs), and,
//! for the app, kept once published (history.rs). Messages for the person
//! go through `tracing` at info and warn: the CLI prints them, the app
//! shows them.

pub mod aec;
pub mod audio;
pub mod brainy;
pub mod history;
pub mod local;
pub mod meeting;
pub mod pending;
pub mod remote;
pub mod series;
pub mod transcript;
pub mod voices;
