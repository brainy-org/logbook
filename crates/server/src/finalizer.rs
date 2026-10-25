//! Final passes' transcription, one job at a time, on a thread of its own.
//!
//! A job is either the ranges a live session's progressive pass asked for
//! (progressive.rs: a minute or so of each channel), answered to the
//! session, or a whole uploaded meeting, whose result goes to `Finals`.
//! Running them one after another bounds what final passes take from the
//! live sessions' GPU to one call at a time, and keeps them off the
//! sessions' threads. An upload's result goes through the model that
//! cleans the transcript up, when there is one and the client did not
//! decline it (polisher.rs).

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use logbook_engine::Engine;
use logbook_engine::final_pass::Recorded;
use logbook_engine::progressive::Range;
use logbook_engine::protocol::{ChannelConfig, FinalState};
use tracing::{info, warn};

use crate::finals::Finals;
use crate::polisher::PolishJob;
use crate::recording::{RangeReader, RecordedFile};
use crate::session::Input;

pub enum FinalJob {
    /// Ranges of a live session's channels, read from its recording.
    Ranges {
        session: String,
        ranges: Vec<(Range, RangeReader)>,
        reply: Sender<Input>,
    },
    /// A meeting a client uploaded, whole.
    Whole {
        id: String,
        channels: Vec<(ChannelConfig, RecordedFile)>,
        /// Through the clean-up model, if there is one.
        polish: bool,
    },
}

pub fn spawn(
    engine: Arc<Engine>,
    finals: Arc<Finals>,
    polisher: Option<Sender<PolishJob>>,
) -> Sender<FinalJob> {
    let (tx, rx) = channel::<FinalJob>();
    std::thread::Builder::new()
        .name("finalizer".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                match job {
                    FinalJob::Ranges {
                        session,
                        ranges,
                        reply,
                    } => run_ranges(&engine, &session, ranges, &reply),
                    FinalJob::Whole {
                        id,
                        channels,
                        polish,
                    } => {
                        let polisher = polisher.as_ref().filter(|_| polish);
                        run_whole(&engine, &finals, polisher, id, channels)
                    }
                }
            }
        })
        .expect("could not start the finalizer thread");
    tx
}

fn run_ranges(
    engine: &Engine,
    session: &str,
    ranges: Vec<(Range, RangeReader)>,
    reply: &Sender<Input>,
) {
    for (range, reader) in ranges {
        let started = Instant::now();
        let result = reader
            .read(range.from, range.to)
            .and_then(|pcm| engine.final_range(&pcm, &range))
            .map_err(|e| format!("{e:#}"));
        match &result {
            Ok(r) => info!(
                session,
                channel = range.channel,
                from = format!("{:.1}", range.from),
                to = format!("{:.1}", range.to),
                words = r.words.len(),
                voices = r.voices.len(),
                took_ms = started.elapsed().as_millis() as u64,
                "final range"
            ),
            Err(error) => warn!(session, "final range failed: {error}"),
        }
        let _ = reply.send(Input::FinalWords { range, result });
    }
}

fn run_whole(
    engine: &Engine,
    finals: &Finals,
    polisher: Option<&Sender<PolishJob>>,
    id: String,
    channels: Vec<(ChannelConfig, RecordedFile)>,
) {
    let started = Instant::now();
    let channels = channels
        .into_iter()
        .map(|(config, file)| {
            let reader = file.reader();
            // The files move into the closure: read, then deleted with it.
            Recorded {
                config,
                audio: Box::new(move || file.read()),
                range: Box::new(move |from, to| reader.read(from, to)),
            }
        })
        .collect();
    let state = match engine.final_pass(channels) {
        Ok(result) => {
            info!(
                utterances = result.utterances.len(),
                took_ms = started.elapsed().as_millis() as u64,
                "final pass of an upload"
            );
            if let Some(polisher) = polisher {
                let job = PolishJob::Whole {
                    id: id.clone(),
                    utterances: result.utterances,
                    warnings: result.warnings,
                };
                match polisher.send(job) {
                    Ok(()) => return,
                    Err(std::sync::mpsc::SendError(job)) => {
                        if let PolishJob::Whole {
                            utterances,
                            warnings,
                            ..
                        } = job
                        {
                            finals.finish(
                                &id,
                                FinalState::Done {
                                    utterances,
                                    warnings,
                                },
                            );
                        }
                        return;
                    }
                }
            }
            FinalState::Done {
                utterances: result.utterances,
                warnings: result.warnings,
            }
        }
        Err(error) => {
            warn!("final pass of an upload failed: {error:#}");
            FinalState::Failed {
                message: format!("the final pass failed: {error:#}"),
            }
        }
    };
    finals.finish(&id, state);
}
