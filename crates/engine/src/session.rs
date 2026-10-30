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
//!
//! A session may take up a meeting another one started (`Resume`): its
//! first sample is at `offset` on the meeting's clock, every time it sends
//! is moved there, and its final pass keeps the meeting's speakers.
//!
//! A session behind the audio (a server on a CPU, where every model's
//! threads take every core) hears from its `Feed`, not its inbox, how far
//! behind it is and whether the client has gone: news in the inbox would
//! wait behind the minutes of audio queued before it. Behind, the
//! progressive pass waits (`HOLD_FINAL`): the live transcript is what the
//! person reads, the final pass can catch up at the end. A client gone
//! before the end of its audio drops the meeting at once: the minutes
//! queued would be transcribed for nobody, while the client takes the
//! meeting up in a new session (remote.rs); so does one gone after it
//! without a final pass, which has nothing left to complete.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::Engine;
use crate::channel::Channel;
use crate::echo::EchoFilter;
use crate::parakeet::Transcript;
use crate::progressive::{Progressive, Range, Transcribed};
use crate::protocol::{
    ChannelConfig, FinalState, SAMPLE_RATE, ServerMessage, SpeakerVoice, Utterance,
};
use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::batcher::Job;
use crate::finalizer::FinalJob;
use crate::finals::Finals;
use crate::recording::Recording;

/// What every session shares.
pub struct Shared {
    pub engine: Arc<Engine>,
    pub batcher: Sender<Job>,
    pub finalizer: Sender<FinalJob>,
    pub finals: Arc<Finals>,
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
    /// The client has no more audio.
    Stop,
    /// The connection is gone: nobody is left to send to.
    Closed,
}

const TICK: Duration = Duration::from_millis(100);

/// Seconds of audio a session may have queued before its progressive pass
/// waits for the live one to catch up; it goes on below half of it, so
/// that it does not stop and start at every frame around the line.
const HOLD_FINAL: f64 = 2.0;

/// What the client's side tells a session beside its inbox: the audio sent
/// and not read yet, the end of the audio, and the client gone. Audio goes
/// through `audio`, the end through `stop`, so that the counts hold.
#[derive(Clone, Default)]
pub struct Feed {
    /// Samples sent and not read yet, every channel together.
    queued: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    gone: Arc<AtomicBool>,
}

impl Feed {
    pub fn audio(&self, inbox: &Sender<Input>, channel: usize, pcm: Vec<f32>) {
        self.queued.fetch_add(pcm.len(), Ordering::Relaxed);
        if inbox.send(Input::Audio(channel, pcm)).is_err() {
            self.queued.store(0, Ordering::Relaxed);
        }
    }

    /// The client has no more audio.
    pub fn stop(&self, inbox: &Sender<Input>) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = inbox.send(Input::Stop);
    }

    /// The connection is gone.
    pub fn close(&self, inbox: &Sender<Input>) {
        self.gone.store(true, Ordering::Relaxed);
        let _ = inbox.send(Input::Closed);
    }

    fn read(&self, samples: usize) {
        self.queued.fetch_sub(samples, Ordering::Relaxed);
    }

    /// Seconds of audio waiting in the inbox.
    fn behind(&self, channels: usize) -> f64 {
        self.queued.load(Ordering::Relaxed) as f64 / (SAMPLE_RATE as f64 * channels.max(1) as f64)
    }

    /// Gone with nothing left to do for it: before the end of its audio,
    /// or with no final pass to complete for `Finals`.
    fn abandoned(&self, final_pass: bool) -> bool {
        self.gone.load(Ordering::Relaxed) && (!self.stopped.load(Ordering::Relaxed) || !final_pass)
    }
}

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

/// The final segments sent, the last `KEEP_SENT` seconds of them, by the
/// part they went in.
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
    /// `KEEP_SENT`. A part goes whole, and
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
            if *until >= latest - KEEP_SENT {
                break;
            }
            self.from = self.from.max(*until);
            self.count = rest;
            self.parts.pop_front();
        }
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
    /// The last segments sent, for `Finals` once the pass is complete.
    sent: Sent,
    warnings: Vec<String>,
    /// The id under which the result is kept, once the meeting has ended.
    id: Option<String>,
    /// The meeting's time of the session's first sample.
    offset: f64,
}

impl Final {
    fn is_done(&self) -> bool {
        self.pass.is_done()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    shared: &Shared,
    session: String,
    configs: Vec<ChannelConfig>,
    final_pass: bool,
    resume: Resume,
    feed: Feed,
    inbox: Receiver<Input>,
    reply: Sender<Input>,
    out: UnboundedSender<ServerMessage>,
) {
    let mut fin = None;
    if final_pass {
        let created =
            Recording::create(&shared.recordings, &session, configs.len()).and_then(|recording| {
                Ok(Final {
                    pass: shared.engine.progressive(configs.clone(), &resume.voices)?,
                    recording,
                    sent: Sent::new(resume.offset),
                    warnings: Vec::new(),
                    id: None,
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
        &feed,
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
    feed: &Feed,
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
    info!(session, channels = ?names, final_pass = fin.is_some(), offset, "session started");
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
    // The progressive pass waits for the live one (`HOLD_FINAL`).
    let mut held = false;
    loop {
        if feed.abandoned(fin.is_some()) {
            info!(session, "the client has gone: the meeting is dropped");
            return Ok(());
        }
        let input = match inbox.recv_timeout(TICK) {
            Ok(input) => Some(input),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let step = (|| -> Result<bool> {
            match input {
                Some(Input::Audio(index, pcm)) if !stopping => {
                    feed.read(pcm.len());
                    if let Some(f) = fin.as_mut() {
                        f.recording.write(index, &pcm)?;
                        f.pass.feed(index, &pcm);
                    }
                    if let Some(channel) = channels.get_mut(index) {
                        submit(channel.feed(&pcm)?);
                    }
                }
                Some(Input::Audio(_, pcm)) => feed.read(pcm.len()),
                None => {}
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
                let behind = feed.behind(channels.len());
                let line = if held { HOLD_FINAL / 2.0 } else { HOLD_FINAL };
                let hold = !stopping && behind > line;
                if hold != held {
                    held = hold;
                    match hold {
                        true => info!(
                            session,
                            behind_s = format!("{behind:.1}"),
                            "the live transcript is behind: the final pass waits"
                        ),
                        false => info!(session, "the final pass goes on"),
                    }
                }
                progress(shared, session, f, &channels, stopping, hold, &reply, out)?;
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
                    let id = shared
                        .finals
                        .start(audio, None)
                        .map_err(|e| format!("{e:#}"))?;
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

/// The progressive pass's turn: quiet points, ranges to transcribe (none
/// while `hold`: the live transcript is behind), and the segments that are
/// ready.
#[allow(clippy::too_many_arguments)]
fn progress(
    shared: &Shared,
    session: &str,
    f: &mut Final,
    channels: &[Channel],
    stopping: bool,
    hold: bool,
    reply: &Sender<Input>,
    out: &UnboundedSender<ServerMessage>,
) -> Result<()> {
    if !stopping {
        for (i, channel) in channels.iter().enumerate() {
            f.pass.quiet(i, channel.quiet_point());
        }
    }
    while let Some((until, utterances)) = f.pass.ready() {
        send(f, until, utterances, out);
        for i in 0..channels.len() {
            f.recording.forget_before(i, f.pass.cut(i));
        }
    }
    if hold {
        return Ok(());
    }
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
        // The end of the meeting keeps them, and says where they start.
        sent.push(f64::MAX, vec![seg(latest)]);
        let (from, all) = sent.take();
        assert!(all.iter().all(|u| u.start >= from));
        assert_eq!(all.first().unwrap().start, from);
        assert_eq!(all.last().unwrap().start, latest);
    }

    #[test]
    fn a_short_session_keeps_everything_from_its_offset() {
        let mut short = Sent::new(12.5);
        short.push(60.0, vec![seg(13.0)]);
        assert_eq!(short.take().0, 12.5);
    }
}
