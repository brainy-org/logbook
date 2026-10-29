//! Final passes' transcription, one job at a time, on a thread of its own.
//!
//! A job is either the ranges a live session's progressive pass asked for
//! (progressive.rs: a minute or so of each channel), answered to the
//! session, or a whole uploaded meeting, whose result goes to `Finals`.
//! Running them one after another bounds what final passes take from the
//! live sessions' GPU to one call at a time, and keeps them off the
//! sessions' threads. A whole meeting is an upload, or one a client kept
//! and reads again in its own process (client local.rs), from where its
//! final segments stopped.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use crate::Engine;
use crate::final_pass::Recorded;
use crate::progressive::Range;
use crate::protocol::{FinalState, SpeakerVoice};
use tracing::{info, warn};

use crate::finals::Finals;
use crate::recording::{RangeReader, RecordedFile};
use crate::session::Input;

pub enum FinalJob {
    /// Ranges of a live session's channels, read from its recording.
    Ranges {
        session: String,
        ranges: Vec<(Range, RangeReader)>,
        reply: Sender<Input>,
    },
    /// A meeting read whole, from where its final segments stopped.
    Whole {
        id: String,
        channels: Vec<Recorded>,
        /// An upload's files, deleted once the pass has read them.
        files: Vec<RecordedFile>,
        /// Where the channels' start is on the meeting's clock.
        offset: f64,
        /// The meeting's speakers before it.
        voices: Vec<SpeakerVoice>,
    },
}

pub fn spawn(engine: Arc<Engine>, finals: Arc<Finals>) -> Sender<FinalJob> {
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
                        files,
                        offset,
                        voices,
                    } => {
                        let state = engine.final_pass(channels, offset, &voices);
                        drop(files);
                        whole_done(&finals, id, state)
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

fn whole_done(finals: &Finals, id: String, result: anyhow::Result<crate::final_pass::Final>) {
    let state = match result {
        Ok(result) => {
            info!(
                utterances = result.utterances.len(),
                "final pass of a whole meeting"
            );
            FinalState::Done {
                utterances: result.utterances,
                warnings: result.warnings,
                voices: result.voices,
                from: result.from,
            }
        }
        Err(error) => {
            warn!("final pass of a whole meeting failed: {error:#}");
            FinalState::Failed {
                message: format!("the final pass failed: {error:#}"),
            }
        }
    };
    finals.finish(&id, state);
}
