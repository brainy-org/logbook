//! Final passes, one at a time, on a thread of their own.
//!
//! A final pass reads a whole meeting: seconds to minutes on the GPU the
//! live sessions use too. Running them one after another bounds what they
//! take from the live sessions to one pass at a time, and keeps them off
//! the sessions' threads.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use logbook_engine::Engine;
use logbook_engine::final_pass::Recorded;
use logbook_engine::protocol::{ChannelConfig, ServerMessage};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::recording::RecordedFile;

pub struct FinalJob {
    pub session: String,
    pub channels: Vec<(ChannelConfig, RecordedFile)>,
    pub out: UnboundedSender<ServerMessage>,
}

pub fn spawn(engine: Arc<Engine>) -> Sender<FinalJob> {
    let (tx, rx) = channel::<FinalJob>();
    std::thread::Builder::new()
        .name("finalizer".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                run(&engine, job);
            }
        })
        .expect("could not start the finalizer thread");
    tx
}

fn run(engine: &Engine, job: FinalJob) {
    let started = Instant::now();
    let channels = job
        .channels
        .into_iter()
        .map(|(config, file)| Recorded {
            config,
            // The file moves into the closure: read, then deleted with it.
            audio: Box::new(move || file.read()),
        })
        .collect();
    let message = match engine.final_pass(channels) {
        Ok(result) => {
            info!(
                session = job.session,
                utterances = result.utterances.len(),
                took_ms = started.elapsed().as_millis() as u64,
                "final pass"
            );
            ServerMessage::Final {
                utterances: result.utterances,
                warnings: result.warnings,
            }
        }
        Err(error) => {
            warn!(session = job.session, "final pass failed: {error:#}");
            ServerMessage::Error {
                message: format!("the final pass failed: {error:#}"),
            }
        }
    };
    let failed = matches!(message, ServerMessage::Error { .. });
    let _ = job.out.send(message);
    if !failed {
        let _ = job.out.send(ServerMessage::Done);
    }
}
