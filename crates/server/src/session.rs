//! One live meeting: a thread that owns its channels.
//!
//! Audio, transcripts and the end of the audio all arrive in one inbox, so
//! the channels are touched by this thread alone. A tick wakes it when
//! nothing arrives, to send what diarization has meanwhile made ready.
//! With a final pass, the audio is also recorded, and the progressive pass
//! (engine progressive.rs) follows the meeting: its ranges are transcribed
//! by the finalizer thread from the recording, and its segments sent as
//! they are ready. After `stop`, the session lives until the pass is
//! complete, even if the client has gone: its result is kept in `Finals`.
//! With a model to clean the transcript up (polisher.rs), and unless the
//! client declined it, the segments go through it before they are sent, a
//! part at a time and in order.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use logbook_engine::Engine;
use logbook_engine::channel::Channel;
use logbook_engine::echo::EchoFilter;
use logbook_engine::parakeet::Transcript;
use logbook_engine::polish::{self, Polished};
use logbook_engine::progressive::{Progressive, Range, Transcribed};
use logbook_engine::protocol::{ChannelConfig, FinalState, ServerMessage, Utterance};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::batcher::Job;
use crate::finalizer::FinalJob;
use crate::finals::Finals;
use crate::polisher::{self, PolishJob};
use crate::recording::Recording;

/// What every session shares.
pub struct Shared {
    pub engine: Arc<Engine>,
    pub batcher: Sender<Job>,
    pub finalizer: Sender<FinalJob>,
    pub finals: Arc<Finals>,
    /// Cleans the final transcript up; None to send it as heard.
    pub polisher: Option<Sender<PolishJob>>,
    /// Where sessions with a final pass record their audio.
    pub recordings: PathBuf,
}

pub enum Input {
    Audio(usize, Vec<f32>),
    Transcribed {
        channel: usize,
        id: u64,
        transcript: Transcript,
    },
    /// A range the progressive pass asked for, read again.
    FinalWords {
        range: Range,
        result: Result<Transcribed, String>,
    },
    /// Final segments back from the model, cleaned where it could.
    Polished {
        until: f64,
        polished: Polished,
    },
    /// The client has no more audio.
    Stop,
    /// The connection is gone: nobody is left to send to.
    Closed,
}

const TICK: Duration = Duration::from_millis(100);

/// A session's final pass.
struct Final {
    pass: Progressive,
    recording: Recording,
    /// Every segment sent, for `Finals` once the pass is complete.
    sent: Vec<Utterance>,
    warnings: Vec<String>,
    /// The id under which the result is kept, once the meeting has ended.
    id: Option<String>,
    /// Segments ready, waiting for the model, in order: (until, them).
    waiting: VecDeque<(f64, Vec<Utterance>)>,
    /// A part is with the model: the next waits for it.
    polishing: bool,
    /// Parts the model could not clean.
    unpolished: Vec<String>,
    /// The model, if the server has one and the client wants it.
    polisher: Option<Sender<PolishJob>>,
}

impl Final {
    fn is_done(&self) -> bool {
        self.pass.is_done() && self.waiting.is_empty() && !self.polishing
    }
}

/// `final_pass`, if asked: whether the model may clean it up.
pub fn run(
    shared: &Shared,
    session: String,
    configs: Vec<ChannelConfig>,
    final_pass: Option<bool>,
    inbox: Receiver<Input>,
    reply: Sender<Input>,
    out: UnboundedSender<ServerMessage>,
) {
    let mut fin = None;
    if let Some(polish) = final_pass {
        let created =
            Recording::create(&shared.recordings, &session, configs.len()).and_then(|recording| {
                Ok(Final {
                    pass: shared.engine.progressive(configs.clone())?,
                    recording,
                    sent: Vec::new(),
                    warnings: Vec::new(),
                    id: None,
                    waiting: VecDeque::new(),
                    polishing: false,
                    unpolished: Vec::new(),
                    polisher: shared.polisher.clone().filter(|_| polish),
                })
            });
        match created {
            Ok(f) => fin = Some(f),
            Err(error) => {
                let _ = out.send(ServerMessage::Error {
                    message: format!("could not start the final pass: {error:#}"),
                });
                return;
            }
        }
    }
    let result = live(shared, &session, configs, &mut fin, inbox, reply, &out);
    // A pass whose meeting ended but which did not complete is failed, not
    // left running for a client that polls it.
    if let Some(Final { id: Some(id), .. }) = &fin
        && let Err(message) = &result
    {
        shared.finals.finish(
            id,
            FinalState::Failed {
                message: message.clone(),
            },
        );
    }
    if let Err(message) = result {
        let _ = out.send(ServerMessage::Error { message });
    }
}

fn live(
    shared: &Shared,
    session: &str,
    configs: Vec<ChannelConfig>,
    fin: &mut Option<Final>,
    inbox: Receiver<Input>,
    reply: Sender<Input>,
    out: &UnboundedSender<ServerMessage>,
) -> Result<(), String> {
    let engine = &shared.engine;
    let batcher = &shared.batcher;
    let mut channels: Vec<Channel> = configs
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, config)| engine.channel(i, config))
        .collect::<Result<_>>()
        .map_err(|e| format!("{e:#}"))?;
    // For each channel that may hear another again: that one's index and the filter.
    let mut echo: Vec<Option<(usize, EchoFilter)>> = Vec::new();
    for (i, config) in configs.iter().enumerate() {
        echo.push(match &config.echo_of {
            None => None,
            Some(name) => match channels.iter().position(|c| c.name() == name) {
                Some(j) if j != i => Some((j, EchoFilter::default())),
                _ => return Err(format!("echo_of names no other channel: {name:?}")),
            },
        });
    }
    let names: Vec<&str> = channels.iter().map(|c| c.name()).collect();
    let polish = fin.as_ref().is_some_and(|f| f.polisher.is_some());
    info!(session, channels = ?names, final_pass = fin.is_some(), polish, "session started");
    let _ = out.send(ServerMessage::Ready);

    let submit = |clips: Vec<logbook_engine::channel::Clip>| {
        for clip in clips {
            let _ = batcher.send(Job {
                clip,
                reply: reply.clone(),
                queued: Instant::now(),
            });
        }
    };
    let mut stopping = false;
    let mut live_done = false;
    loop {
        let input = match inbox.recv_timeout(TICK) {
            Ok(input) => Some(input),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let step = (|| -> Result<bool> {
            match input {
                Some(Input::Audio(index, pcm)) if !stopping => {
                    if let Some(f) = fin.as_mut() {
                        f.recording.write(index, &pcm)?;
                        f.pass.feed(index, &pcm);
                    }
                    if let Some(channel) = channels.get_mut(index) {
                        submit(channel.feed(&pcm)?);
                    }
                }
                Some(Input::Audio(..)) | None => {}
                Some(Input::Transcribed {
                    channel,
                    id,
                    transcript,
                }) => {
                    if let Some(channel) = channels.get_mut(channel) {
                        channel.transcribed(id, transcript);
                    }
                }
                Some(Input::FinalWords { range, result }) => {
                    if let Some(f) = fin.as_mut() {
                        let result = result.unwrap_or_else(|error| {
                            f.warnings.push(format!(
                                "{:.0}–{:.0} s of {} was not transcribed again: {error}",
                                range.from,
                                range.to,
                                channels[range.channel].name()
                            ));
                            Transcribed::default()
                        });
                        f.pass.transcribed(&range, result);
                    }
                }
                Some(Input::Polished { until, polished }) => {
                    if let Some(f) = fin.as_mut() {
                        f.polishing = false;
                        f.unpolished.extend(polished.errors);
                        send(f, until, polished.utterances, out);
                    }
                }
                Some(Input::Stop) => {
                    if !stopping {
                        stopping = true;
                        for channel in &mut channels {
                            submit(channel.finish()?);
                        }
                    }
                }
                // Before the end, the meeting is dropped with the client;
                // after it, its final pass completes for `Finals`.
                Some(Input::Closed) => return Ok(stopping && fin.is_some()),
            }
            for i in 0..channels.len() {
                for utterance in channels[i].ready()? {
                    for (source, filter) in echo.iter_mut().flatten() {
                        if *source == i {
                            filter.heard(&utterance);
                        }
                    }
                    match &mut echo[i] {
                        Some((_, filter)) => filter.hold(utterance),
                        None => {
                            let _ = out.send(ServerMessage::Utterance(utterance));
                        }
                    }
                }
            }
            for (source, filter) in echo.iter_mut().flatten() {
                for utterance in filter.release(channels[*source].settled()) {
                    let _ = out.send(ServerMessage::Utterance(utterance));
                }
                // The meeting's words: only at debug (RUST_LOG=debug).
                for seen in filter.take_seen() {
                    debug!(session, "echo: {seen}");
                }
            }
            if let Some(f) = fin.as_mut() {
                progress(shared, session, f, &channels, stopping, &reply, out)?;
            }
            Ok(true)
        })();
        match step {
            Ok(true) => {}
            Ok(false) => {
                info!(session, "session closed by the client");
                return Ok(());
            }
            Err(error) => {
                warn!(session, "session failed: {error:#}");
                return Err(format!("{error:#}"));
            }
        }
        if stopping
            && !live_done
            && channels.iter().all(Channel::idle)
            && echo.iter().flatten().all(|(_, f)| f.is_empty())
        {
            live_done = true;
            let audio = channels.iter().map(|c| c.time()).fold(0.0, f64::max);
            info!(
                session,
                audio_s = format!("{audio:.1}"),
                "live transcript done"
            );
            match fin.as_mut() {
                None => {
                    let _ = out.send(ServerMessage::Done);
                    return Ok(());
                }
                Some(f) => {
                    f.pass.finish();
                    let id = shared.finals.start(audio).map_err(|e| format!("{e:#}"))?;
                    f.id = Some(id.clone());
                    let _ = out.send(ServerMessage::Finalizing { audio_s: audio, id });
                }
            }
        }
        if live_done
            && let Some(f) = fin.as_mut()
            && f.is_done()
        {
            info!(session, segments = f.sent.len(), "final pass done");
            f.warnings.extend(polisher::warning(&f.unpolished));
            let _ = out.send(ServerMessage::Final {
                utterances: Vec::new(),
                warnings: f.warnings.clone(),
            });
            let _ = out.send(ServerMessage::Done);
            if let Some(id) = &f.id {
                shared.finals.finish(
                    id,
                    FinalState::Done {
                        utterances: std::mem::take(&mut f.sent),
                        warnings: f.warnings.clone(),
                    },
                );
            }
            return Ok(());
        }
    }
}

/// The progressive pass's turn: quiet points, ranges to transcribe, and the
/// segments that are ready.
fn progress(
    shared: &Shared,
    session: &str,
    f: &mut Final,
    channels: &[Channel],
    stopping: bool,
    reply: &Sender<Input>,
    out: &UnboundedSender<ServerMessage>,
) -> Result<()> {
    if !stopping {
        for (i, channel) in channels.iter().enumerate() {
            f.pass.quiet(i, channel.quiet_point());
        }
    }
    while let Some(ready) = f.pass.ready() {
        f.waiting.push_back(ready);
        for i in 0..channels.len() {
            f.recording.forget_before(i, f.pass.cut(i));
        }
    }
    release(session, f, reply, out);
    let ranges = f.pass.next_ranges();
    if !ranges.is_empty() {
        f.recording.flush()?;
        let ranges = ranges
            .into_iter()
            .filter_map(|r| f.recording.reader(r.channel).map(|reader| (r, reader)))
            .collect();
        let _ = shared.finalizer.send(FinalJob::Ranges {
            session: session.to_string(),
            ranges,
            reply: reply.clone(),
        });
    }
    Ok(())
}

/// The segments waiting, in order: through the model when there is one, a
/// part at a time (the next one's context is this one cleaned); straight
/// out otherwise.
fn release(
    session: &str,
    f: &mut Final,
    reply: &Sender<Input>,
    out: &UnboundedSender<ServerMessage>,
) {
    while !f.polishing {
        let Some((until, utterances)) = f.waiting.pop_front() else {
            return;
        };
        let Some(polisher) = f.polisher.as_ref().filter(|_| !utterances.is_empty()) else {
            send(f, until, utterances, out);
            continue;
        };
        let context = f.sent[f.sent.len().saturating_sub(polish::CONTEXT)..].to_vec();
        let job = PolishJob::Segments {
            session: session.to_string(),
            context,
            until,
            utterances,
            reply: reply.clone(),
        };
        match polisher.send(job) {
            Ok(()) => f.polishing = true,
            // No polisher left: as heard.
            Err(std::sync::mpsc::SendError(job)) => {
                if let PolishJob::Segments {
                    until, utterances, ..
                } = job
                {
                    f.unpolished
                        .push(format!("{until:.0} s: the polisher has stopped"));
                    send(f, until, utterances, out);
                }
            }
        }
    }
}

/// Final segments out to the client, and kept for `Finals`.
fn send(
    f: &mut Final,
    until: f64,
    utterances: Vec<Utterance>,
    out: &UnboundedSender<ServerMessage>,
) {
    f.sent.extend(utterances.iter().cloned());
    let _ = out.send(ServerMessage::FinalSegments { until, utterances });
}
