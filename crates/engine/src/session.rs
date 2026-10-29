//! One live meeting: a thread that owns its channels.
//!
//! Audio, transcripts and the end of the audio all arrive in one inbox, so
//! the channels are touched by this thread alone. A tick wakes it when
//! nothing arrives, to send what diarization has meanwhile made ready.
//! With a final pass, the audio is also recorded, and the progressive pass
//! (engine progressive.rs) follows the meeting: its ranges are transcribed
//! by the finalizer thread from the recording, and its segments sent as
//! they are ready. After `stop`, the session lives until the pass is
//! complete, even if the client has gone: its result is kept in `Finals`,
//! the segments of its last minutes (`Sent`, `KEEP_SENT`): a session
//! holds minutes of segments, not the meeting's.
//! With a model to clean the transcript up (polisher.rs), and unless the
//! client declined it, the segments go through it before they are sent, a
//! part at a time and in order.
//!
//! A session may take up a meeting another one started (`Resume`): its
//! first sample is at `offset` on the meeting's clock, every time it sends
//! is moved there, and its final pass keeps the meeting's speakers.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::Engine;
use crate::channel::Channel;
use crate::echo::EchoFilter;
use crate::parakeet::Transcript;
use crate::polish::{self, Polished};
use crate::progressive::{Progressive, Range, Transcribed};
use crate::protocol::{ChannelConfig, FinalState, ServerMessage, SpeakerVoice, Utterance};
use anyhow::Result;
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

/// Where a session starts in its meeting (protocol.rs, `Start`).
#[derive(Clone, Debug, Default)]
pub struct Resume {
    /// The meeting's time of the session's first sample, in seconds.
    pub offset: f64,
    /// The meeting's speakers so far.
    pub voices: Vec<SpeakerVoice>,
}

/// How much of the meeting's final segments a session keeps once sent:
/// for a client that lost the end of the pass (`Finals`, `from`), which
/// has every segment before its last `until`, minutes behind at most.
pub const KEEP_SENT: f64 = 600.0;

/// The final segments sent, the last `KEEP_SENT` seconds of them (and at
/// least `polish::CONTEXT`), by the part they went in.
#[derive(Debug, Default)]
pub struct Sent {
    /// (until, segments), on the meeting's clock, in the order sent.
    parts: VecDeque<(f64, Vec<Utterance>)>,
    /// Segments starting at or after this are all kept.
    from: f64,
    count: usize,
}

impl Sent {
    /// Nothing sent yet; the session's segments start at `from` (its
    /// offset on the meeting's clock).
    pub fn new(from: f64) -> Self {
        Self {
            parts: VecDeque::new(),
            from,
            count: 0,
        }
    }

    /// A part sent with `until`; the oldest parts go once the rest covers
    /// `KEEP_SENT` and the clean-up's context. A part goes whole, and
    /// `from` becomes its `until`: every segment from there on is kept.
    pub fn push(&mut self, until: f64, utterances: Vec<Utterance>) {
        self.count += utterances.len();
        self.parts.push_back((until, utterances));
        // The end of the meeting (f64::MAX) keeps the last finite one.
        let Some(latest) = self
            .parts
            .iter()
            .rev()
            .map(|p| p.0)
            .find(|&u| u != f64::MAX)
        else {
            return;
        };
        while let Some((until, front)) = self.parts.front() {
            let rest = self.count - front.len();
            if *until >= latest - KEEP_SENT || rest < polish::CONTEXT {
                break;
            }
            self.from = self.from.max(*until);
            self.count = rest;
            self.parts.pop_front();
        }
    }

    /// The last `n` segments sent.
    pub fn last(&self, n: usize) -> Vec<Utterance> {
        let mut out: Vec<Utterance> = self
            .parts
            .iter()
            .rev()
            .flat_map(|p| p.1.iter().rev())
            .take(n)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Where the kept segments start (`FinalState::Done::from`).
    pub fn from(&self) -> f64 {
        self.from
    }

    /// The kept segments, by start, and `from`.
    pub fn take(&mut self) -> (f64, Vec<Utterance>) {
        let all = std::mem::take(&mut self.parts)
            .into_iter()
            .flat_map(|p| p.1)
            .collect();
        self.count = 0;
        (self.from, all)
    }
}

/// A session's final pass.
struct Final {
    pass: Progressive,
    recording: Recording,
    /// The last segments sent, for the clean-up's context and for
    /// `Finals` once the pass is complete.
    sent: Sent,
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
    /// The meeting's time of the session's first sample.
    offset: f64,
}

impl Final {
    fn is_done(&self) -> bool {
        self.pass.is_done() && self.waiting.is_empty() && !self.polishing
    }
}

/// `final_pass`, if asked: whether the model may clean it up.
#[allow(clippy::too_many_arguments)]
pub fn run(
    shared: &Shared,
    session: String,
    configs: Vec<ChannelConfig>,
    final_pass: Option<bool>,
    resume: Resume,
    inbox: Receiver<Input>,
    reply: Sender<Input>,
    out: UnboundedSender<ServerMessage>,
) {
    let mut fin = None;
    if let Some(polish) = final_pass {
        let created =
            Recording::create(&shared.recordings, &session, configs.len()).and_then(|recording| {
                Ok(Final {
                    pass: shared.engine.progressive(configs.clone(), &resume.voices)?,
                    recording,
                    sent: Sent::new(resume.offset),
                    warnings: Vec::new(),
                    id: None,
                    waiting: VecDeque::new(),
                    polishing: false,
                    unpolished: Vec::new(),
                    polisher: shared.polisher.clone().filter(|_| polish),
                    offset: resume.offset,
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
    let result = live(
        shared,
        &session,
        configs,
        resume.offset,
        &mut fin,
        inbox,
        reply,
        &out,
    );
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

#[allow(clippy::too_many_arguments)]
fn live(
    shared: &Shared,
    session: &str,
    configs: Vec<ChannelConfig>,
    offset: f64,
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
    info!(session, channels = ?names, final_pass = fin.is_some(), polish, offset, "session started");
    let _ = out.send(ServerMessage::Ready { codec: None });

    let submit = |clips: Vec<crate::channel::Clip>| {
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
                            let _ = out.send(ServerMessage::Utterance(utterance.shifted(offset)));
                        }
                    }
                }
            }
            for (source, filter) in echo.iter_mut().flatten() {
                for utterance in filter.release(channels[*source].settled()) {
                    let _ = out.send(ServerMessage::Utterance(utterance.shifted(offset)));
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
            let voices = f.pass.voices();
            let _ = out.send(ServerMessage::Final {
                utterances: Vec::new(),
                warnings: f.warnings.clone(),
                voices: voices.clone(),
            });
            let _ = out.send(ServerMessage::Done);
            if let Some(id) = &f.id {
                let (from, utterances) = f.sent.take();
                shared.finals.finish(
                    id,
                    FinalState::Done {
                        utterances,
                        warnings: f.warnings.clone(),
                        voices,
                        from,
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
        let context = f.sent.last(polish::CONTEXT);
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

/// Final segments out to the client, and kept for `Finals`, on the
/// meeting's clock.
fn send(
    f: &mut Final,
    until: f64,
    utterances: Vec<Utterance>,
    out: &UnboundedSender<ServerMessage>,
) {
    let utterances: Vec<Utterance> = utterances
        .into_iter()
        .map(|u| u.shifted(f.offset))
        .collect();
    // f64::MAX ("everything") stays as it is.
    let until = if until == f64::MAX {
        until
    } else {
        until + f.offset
    };
    f.sent.push(until, utterances.clone());
    // Every speaker's voice as it stands: a few kB a minute, and the client
    // names a speaker from it before the segment goes on the page.
    let _ = out.send(ServerMessage::FinalSegments {
        until,
        utterances,
        voices: f.pass.voices(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64) -> Utterance {
        Utterance {
            channel: "mic".into(),
            speaker: None,
            start,
            end: start + 5.0,
            text: format!("{start}"),
            words: Vec::new(),
        }
    }

    #[test]
    fn a_session_keeps_the_last_minutes_it_sent() {
        let mut sent = Sent::new(30.0);
        // Three hours, a part a minute, two segments each.
        for minute in 1..=180 {
            let until = 30.0 + minute as f64 * 60.0;
            sent.push(until, vec![seg(until - 60.0), seg(until - 30.0)]);
        }
        // About ten minutes kept, from a part's until.
        let latest = 30.0 + 180.0 * 60.0;
        assert!(sent.len() <= 2 * (KEEP_SENT as usize / 60 + 1));
        assert!(sent.from() >= latest - KEEP_SENT - 60.0 && sent.from() <= latest - KEEP_SENT);
        assert_eq!(sent.last(polish::CONTEXT).len(), polish::CONTEXT);
        assert_eq!(sent.last(1)[0].start, latest - 30.0);
        // The end of the meeting keeps them, and says where they start.
        sent.push(f64::MAX, vec![seg(latest)]);
        let (from, all) = sent.take();
        assert!(all.iter().all(|u| u.start >= from));
        assert_eq!(all.first().unwrap().start, from);
        assert_eq!(all.last().unwrap().start, latest);
    }

    #[test]
    fn the_clean_up_s_context_is_kept_through_a_long_silence() {
        let mut sent = Sent::new(0.0);
        sent.push(60.0, vec![seg(10.0), seg(20.0), seg(30.0)]);
        // An hour of parts with nothing in them.
        for minute in 2..=60 {
            sent.push(minute as f64 * 60.0, Vec::new());
        }
        assert_eq!(sent.last(polish::CONTEXT).len(), 3);
        assert_eq!(sent.from(), 0.0);
        // A short session: everything, from its offset.
        let mut short = Sent::new(12.5);
        short.push(60.0, vec![seg(13.0)]);
        assert_eq!(short.take().0, 12.5);
    }
}
