//! The final transcript cleaned up by a language model (engine polish.rs),
//! on threads of their own: a request takes seconds, and the finalizer,
//! which every session's pass waits on, must not wait on it. A live
//! session has one part out at a time (session.rs), so its segments leave
//! in order and each part follows the last one cleaned; the threads are
//! how many sessions' parts the model is asked for at once.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::polish::{Polished, Polisher};
use crate::protocol::{FinalState, SpeakerVoice, Utterance};
use tracing::{info, warn};

use crate::finals::Finals;
use crate::session::Input;

pub enum PolishJob {
    /// A live session's final segments, answered to the session.
    Segments {
        session: String,
        /// The last segments sent, already cleaned.
        context: Vec<Utterance>,
        until: f64,
        utterances: Vec<Utterance>,
        reply: Sender<Input>,
    },
    /// An uploaded meeting's final pass, whose result goes to `Finals`.
    Whole {
        id: String,
        utterances: Vec<Utterance>,
        warnings: Vec<String>,
        voices: Vec<SpeakerVoice>,
    },
}

pub fn spawn(polisher: Polisher, finals: Arc<Finals>, threads: usize) -> Sender<PolishJob> {
    let (tx, rx) = channel::<PolishJob>();
    let rx = Arc::new(Mutex::new(rx));
    let polisher = Arc::new(polisher);
    for n in 0..threads.max(1) {
        let (rx, polisher, finals) = (rx.clone(), polisher.clone(), finals.clone());
        std::thread::Builder::new()
            .name(format!("polisher-{n}"))
            .spawn(move || work(&rx, &polisher, &finals))
            .expect("could not start a polisher thread");
    }
    tx
}

fn work(rx: &Mutex<Receiver<PolishJob>>, polisher: &Polisher, finals: &Finals) {
    loop {
        // The lock is held only while waiting for the next job.
        let Ok(job) = rx.lock().unwrap().recv() else {
            return;
        };
        match job {
            PolishJob::Segments {
                session,
                context,
                until,
                utterances,
                reply,
            } => {
                let polished = timed(polisher, &session, &context, utterances);
                let _ = reply.send(Input::Polished { until, polished });
            }
            PolishJob::Whole {
                id,
                utterances,
                mut warnings,
                voices,
            } => {
                let polished = timed(polisher, "upload", &[], utterances);
                warnings.extend(warning(&polished.errors));
                finals.finish(
                    &id,
                    FinalState::Done {
                        utterances: polished.utterances,
                        warnings,
                        voices,
                    },
                );
            }
        }
    }
}

fn timed(
    polisher: &Polisher,
    session: &str,
    context: &[Utterance],
    utterances: Vec<Utterance>,
) -> Polished {
    let started = Instant::now();
    let segments = utterances.len();
    let polished = polisher.polish(context, utterances);
    // Counts and times only: the text is the meeting's.
    info!(
        session,
        segments,
        kept = polished.utterances.len(),
        failed = polished.errors.len(),
        took_ms = started.elapsed().as_millis() as u64,
        "polished"
    );
    for error in &polished.errors {
        warn!(session, "not polished: {error}");
    }
    polished
}

/// What the meeting's `warnings` say of the parts the model did not clean.
pub fn warning(errors: &[String]) -> Option<String> {
    let first = errors.first()?;
    Some(format!(
        "{} part(s) of the transcript were not cleaned up and are as heard; the first: {first}",
        errors.len()
    ))
}
