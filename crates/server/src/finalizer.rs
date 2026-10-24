//! Final passes, one at a time, on a thread of their own.
//!
//! A final pass reads a whole meeting: seconds to minutes on the GPU the
//! live sessions use too. Running them one after another bounds what they
//! take from the live sessions to one pass at a time, and keeps them off
//! the sessions' threads. The result goes to `Finals` under the pass's id,
//! and to the live session's connection if there is one still open.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use logbook_engine::Engine;
use logbook_engine::final_pass::Recorded;
use logbook_engine::protocol::{ChannelConfig, FinalState, ServerMessage};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::finals::Finals;
use crate::recording::RecordedFile;

pub struct FinalJob {
    /// What the logs call it: the live session, or "upload".
    pub label: String,
    pub id: String,
    pub channels: Vec<(ChannelConfig, RecordedFile)>,
    /// The live session's connection; None for an upload.
    pub out: Option<UnboundedSender<ServerMessage>>,
}

pub fn spawn(engine: Arc<Engine>, finals: Arc<Finals>) -> Sender<FinalJob> {
    let (tx, rx) = channel::<FinalJob>();
    std::thread::Builder::new()
        .name("finalizer".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                run(&engine, &finals, job);
            }
        })
        .expect("could not start the finalizer thread");
    tx
}

fn run(engine: &Engine, finals: &Finals, job: FinalJob) {
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
    let state = match engine.final_pass(channels) {
        Ok(result) => {
            info!(
                session = job.label,
                utterances = result.utterances.len(),
                took_ms = started.elapsed().as_millis() as u64,
                "final pass"
            );
            FinalState::Done {
                utterances: result.utterances,
                warnings: result.warnings,
            }
        }
        Err(error) => {
            warn!(session = job.label, "final pass failed: {error:#}");
            FinalState::Failed {
                message: format!("the final pass failed: {error:#}"),
            }
        }
    };
    if let Some(out) = &job.out {
        match &state {
            FinalState::Done {
                utterances,
                warnings,
            } => {
                let _ = out.send(ServerMessage::Final {
                    utterances: utterances.clone(),
                    warnings: warnings.clone(),
                });
                let _ = out.send(ServerMessage::Done);
            }
            FinalState::Failed { message } => {
                let _ = out.send(ServerMessage::Error {
                    message: message.clone(),
                });
            }
            FinalState::Running { .. } => {}
        }
    }
    finals.finish(&job.id, state);
}
