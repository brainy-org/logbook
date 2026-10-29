//! A recording that may go on for hours or months, cut into meetings
//! (docs/decisions/2026-10-29-bounded-meetings.md): each one is a meeting
//! of meeting.rs, with its own session, pending folder, page and report,
//! and what each keeps is bounded by its length.
//!
//! The capture never stops. A forwarder (`Cutter`) passes every frame to
//! the meeting being recorded; when it closes (`Closer`: a pause in the
//! live transcript, or its length), every channel is cut at one sample:
//! what comes before goes to it, what comes after to the next, a frame
//! across the cut split in two. Channels stay on one clock in each
//! meeting, which the echo filter needs (it compares them by time). The
//! closed meeting ends as a meeting ends when the recording stops (its
//! final pass, page, report), while the next one records.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Local;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::protocol::{ChannelConfig, Utterance};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::meeting::{self, Backend, Notes, Outcome, Publish};
use crate::remote::{self, Audio};

/// A meeting closes once no live phrase has ended for this long.
pub const PAUSE: f64 = 15.0 * 60.0;
/// A meeting this long closes at the end of its next live phrase...
pub const MAX_LENGTH: f64 = 8.0 * 3600.0;
/// ...or this much later in any case: no phrase ends in continuous noise,
/// and none comes without a server.
pub const GRACE: f64 = 2.0 * 60.0;

/// When a recording is cut into meetings; None never.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Seconds without a live phrase ending that close a meeting.
    pub pause: Option<f64>,
    /// Seconds of a meeting after which it closes at its next phrase's
    /// end, and `GRACE` later at the latest.
    pub max_length: Option<f64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            pause: Some(PAUSE),
            max_length: Some(MAX_LENGTH),
        }
    }
}

impl Limits {
    /// A pause in minutes and a length in hours, as the CLI's `--pause`
    /// and `--max-length` take them; 0 (or less) for never.
    pub fn new(pause_minutes: f64, max_length_hours: f64) -> Self {
        let positive = |x: f64| Some(x).filter(|&x| x > 0.0);
        Self {
            pause: positive(pause_minutes * 60.0),
            max_length: positive(max_length_hours * 3600.0),
        }
    }

    /// The defaults, or `LOGBOOK_PAUSE` (minutes) and `LOGBOOK_MAX_LENGTH`
    /// (hours) where set: the app has no settings for them, and a short
    /// pause tries a cut in a minute.
    pub fn from_env() -> Self {
        let var = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(default)
        };
        Self::new(
            var("LOGBOOK_PAUSE", PAUSE / 60.0),
            var("LOGBOOK_MAX_LENGTH", MAX_LENGTH / 3600.0),
        )
    }
}

/// Whether the meeting being recorded is over. Times are seconds of the
/// meeting (its samples): no clock of its own, so a test gives it any.
#[derive(Debug)]
pub struct Closer {
    limits: Limits,
    /// A live session is open: its silence is the meeting's.
    connected: bool,
    /// The pause counts from here: the last phrase's end, or when the
    /// live transcript came (back), or the meeting's start.
    quiet_since: f64,
}

impl Closer {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            connected: false,
            quiet_since: 0.0,
        }
    }

    /// The live transcript runs (or stops) from `now`. Offline, nothing
    /// is transcribed live, so silence says nothing of the meeting.
    pub fn connected(&mut self, up: bool, now: f64) {
        if up && !self.connected {
            self.quiet_since = self.quiet_since.max(now);
        }
        self.connected = up;
    }

    /// A live phrase of the meeting ended at `end`, heard at `now`: true
    /// if the meeting closes here (it is past its length).
    pub fn heard(&mut self, end: f64, now: f64) -> bool {
        self.quiet_since = self.quiet_since.max(end);
        self.limits.max_length.is_some_and(|max| now >= max)
    }

    /// The meeting closes at `now`: a pause while the live transcript
    /// runs, or its length and the grace after it.
    pub fn due(&self, now: f64) -> bool {
        let paused = self
            .limits
            .pause
            .is_some_and(|pause| self.connected && now - self.quiet_since >= pause);
        let long = self.limits.max_length.is_some_and(|max| now >= max + GRACE);
        paused || long
    }
}

/// Every channel's frames to the meeting before a cut or after it, the
/// cut at one sample for all.
#[derive(Debug)]
pub struct Cutter {
    /// Per channel, samples passed so far, on the recording's clock.
    position: Vec<u64>,
    /// A cut some channel has not reached yet.
    cut: Option<u64>,
}

impl Cutter {
    pub fn new(channels: usize) -> Self {
        Self {
            position: vec![0; channels],
            cut: None,
        }
    }

    /// The furthest channel's position: the recording's length so far.
    pub fn position(&self) -> u64 {
        self.position.iter().copied().max().unwrap_or(0)
    }

    /// Cuts every channel where the furthest one is (none can go back);
    /// returns that sample. The ones behind send what they still have
    /// before it to the meeting closing.
    pub fn cut(&mut self) -> u64 {
        let at = self.position();
        self.cut = self.position.iter().any(|&p| p < at).then_some(at);
        at
    }

    /// A cut is waiting for some channel to reach it.
    pub fn cutting(&self) -> bool {
        self.cut.is_some()
    }

    /// A frame of `channel`: (before the cut, after it). Without a cut
    /// waiting, all of it is after (the meeting being recorded). Moved,
    /// not copied; split in two (one allocation) across the cut.
    pub fn route(
        &mut self,
        channel: usize,
        mut samples: Vec<i16>,
    ) -> (Option<Vec<i16>>, Option<Vec<i16>>) {
        let Some(position) = self.position.get_mut(channel) else {
            return (None, Some(samples));
        };
        let start = *position;
        let end = start + samples.len() as u64;
        *position = end;
        let routed = match self.cut {
            Some(cut) if start < cut => {
                if end <= cut {
                    (Some(samples), None)
                } else {
                    let after = samples.split_off((cut - start) as usize);
                    (Some(samples), Some(after))
                }
            }
            _ => (None, Some(samples)),
        };
        if let Some(cut) = self.cut
            && self.position.iter().all(|&p| p >= cut)
        {
            self.cut = None;
        }
        routed
    }
}

/// What a series of meetings tells its caller, beyond a meeting's events
/// (whose times are on the recording's clock: the first meeting's).
pub trait Series: remote::Events {
    /// Meeting `n` (the first is 0) started at `at` seconds of the
    /// recording: the one before it is closing. Not said of the first.
    fn meeting_started(&mut self, _n: usize, _at: f64) {}
    /// Meeting `n` came to its end while the recording went on, or after
    /// it while a later one was finishing. The last one's is the result
    /// of `meetings`.
    fn meeting_ended(&mut self, _n: usize, _outcome: &Result<Option<Outcome>>) {}
}

enum Note {
    Utterance(Utterance),
    Finalizing(f64, String),
    Connected(bool),
    Ended(Result<Option<Outcome>>),
}

/// A meeting's events, to the series' loop.
struct Forward {
    n: usize,
    tx: UnboundedSender<(usize, Note)>,
}

impl remote::Events for Forward {
    fn utterance(&mut self, u: Utterance) {
        let _ = self.tx.send((self.n, Note::Utterance(u)));
    }

    fn finalizing(&mut self, audio_s: f64, id: &str) {
        let _ = self
            .tx
            .send((self.n, Note::Finalizing(audio_s, id.to_string())));
    }

    fn connected(&mut self, up: bool) {
        let _ = self.tx.send((self.n, Note::Connected(up)));
    }
}

/// The notes of a series' meetings, by number: each is written to the
/// meeting it was typed in, never to the one current when it arrives. A
/// meeting reads its notes once its final pass is done (meeting.rs), so
/// the last keystrokes before a cut, sent just after it, still reach the
/// meeting that closed. Kept while the meeting runs.
#[derive(Clone, Default)]
pub struct Notebook(Arc<Mutex<HashMap<usize, Notes>>>);

impl Notebook {
    /// Meeting `n`'s notes are `text`; nothing if it has ended.
    pub fn set(&self, n: usize, text: String) {
        if let Some(notes) = self.0.lock().unwrap().get(&n) {
            *notes.lock().unwrap() = text;
        }
    }

    /// Meeting `n`'s notes so far.
    pub fn get(&self, n: usize) -> String {
        self.0
            .lock()
            .unwrap()
            .get(&n)
            .map(|notes| notes.lock().unwrap().clone())
            .unwrap_or_default()
    }

    fn open(&self, n: usize) -> Notes {
        self.0.lock().unwrap().entry(n).or_default().clone()
    }

    fn close(&self, n: usize) {
        self.0.lock().unwrap().remove(&n);
    }
}

/// A meeting of the series being recorded.
struct Part {
    n: usize,
    /// Its sample 0 on the recording's clock.
    base: u64,
    audio: Option<UnboundedSender<(u8, Vec<i16>)>>,
}

/// The audio of `audio` as a series of meetings (`Limits`), each run as
/// `meeting::meeting` runs one; `notes` holds the notes being written,
/// by meeting. Returns the last meeting's
/// outcome once every meeting has ended; the others go to
/// `Series::meeting_ended`. A meeting that ends before its audio does
/// (its backend failed) ends the series.
#[allow(clippy::too_many_arguments)]
pub async fn meetings(
    backend: &Backend,
    data: &Path,
    channels: Vec<ChannelConfig>,
    p: &Publish,
    mut audio: Audio,
    notes: &Notebook,
    limits: Limits,
    events: &mut impl Series,
) -> Result<Option<Outcome>> {
    let started = Local::now();
    let rate = SAMPLE_RATE as f64;
    let (note_tx, mut note_rx) = unbounded_channel::<(usize, Note)>();
    let start = |n: usize, base: u64| -> Part {
        let (tx, rx) = unbounded_channel();
        let (backend, data, channels) = (backend.clone(), data.to_path_buf(), channels.clone());
        let mut p = p.clone();
        if n > 0 {
            p.title = p
                .title
                .filter(|t| !t.trim().is_empty())
                .map(|t| format!("{t} ({})", n + 1));
        }
        let at = started + chrono::Duration::milliseconds((base as f64 / rate * 1000.0) as i64);
        let notes = notes.open(n);
        let mut forward = Forward {
            n,
            tx: note_tx.clone(),
        };
        tokio::spawn(async move {
            let result =
                meeting::meeting_at(&backend, &data, channels, &p, rx, notes, at, &mut forward)
                    .await;
            let _ = forward.tx.send((n, Note::Ended(result)));
        });
        Part {
            n,
            base,
            audio: Some(tx),
        }
    };
    let mut current = start(0, 0);
    let mut closing: Option<Part> = None;
    // Each running meeting's sample 0, by number.
    let mut bases: HashMap<usize, u64> = HashMap::from([(0, 0)]);
    let mut running = 1;
    let mut cutter = Cutter::new(channels.len());
    let mut closer = Closer::new(limits);
    let mut ended = false;
    let mut last: Option<Result<Option<Outcome>>> = None;
    // A meeting being recorded closes: the next one starts at the cut.
    macro_rules! cut {
        () => {{
            let at = cutter.cut();
            let next = start(current.n + 1, at);
            bases.insert(next.n, at);
            running += 1;
            closing = Some(std::mem::replace(&mut current, next));
            closer = Closer::new(limits);
            events.meeting_started(current.n, at as f64 / rate);
        }};
    }
    while running > 0 {
        tokio::select! {
            frame = audio.recv(), if !ended => match frame {
                Some((channel, samples)) => {
                    let (before, after) = cutter.route(channel as usize, samples);
                    if let (Some(samples), Some(tx)) =
                        (before, closing.as_ref().and_then(|c| c.audio.as_ref()))
                    {
                        let _ = tx.send((channel, samples));
                    }
                    if let (Some(samples), Some(tx)) = (after, current.audio.as_ref()) {
                        let _ = tx.send((channel, samples));
                    }
                    let now = (cutter.position() - current.base) as f64 / rate;
                    if closing.is_none() && closer.due(now) {
                        cut!();
                    }
                }
                None => {
                    ended = true;
                    current.audio = None;
                    closing = None;
                }
            },
            note = note_rx.recv() => {
                // `start` keeps a sender: never None.
                let Some((n, note)) = note else { break };
                let base = bases.get(&n).copied().unwrap_or(0);
                let now = (cutter.position().saturating_sub(current.base)) as f64 / rate;
                match note {
                    Note::Utterance(u) => {
                        let closes = n == current.n && closer.heard(u.end, now);
                        events.utterance(u.shifted(base as f64 / rate));
                        if closes && !ended && closing.is_none() {
                            cut!();
                        }
                    }
                    Note::Finalizing(audio_s, id) => events.finalizing(audio_s, &id),
                    Note::Connected(up) => {
                        if n == current.n {
                            closer.connected(up, now);
                        }
                        events.connected(up);
                    }
                    Note::Ended(result) => {
                        running -= 1;
                        bases.remove(&n);
                        notes.close(n);
                        if n == current.n {
                            if !ended {
                                // Its backend failed before the audio
                                // ended: the series ends with it.
                                ended = true;
                                audio.close();
                                current.audio = None;
                                closing = None;
                            }
                            last = Some(result);
                        } else {
                            events.meeting_ended(n, &result);
                        }
                    }
                }
            }
        }
        if closing.is_some() && !cutter.cutting() {
            // Every channel has reached the cut: its audio ends.
            closing = None;
        }
    }
    last.unwrap_or(Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `n` samples of `channel` (each its position, as a number)
    /// and notes which meeting each part goes to, as `meetings` sends
    /// them: before the cut to the one closing, after it to `current`.
    fn feed(
        c: &mut Cutter,
        sent: &mut [Vec<(usize, Vec<i16>)>],
        current: usize,
        ch: usize,
        n: usize,
    ) {
        let first = c.position[ch] as i16;
        let frame: Vec<i16> = (0..n).map(|i| first + i as i16).collect();
        let (before, after) = c.route(ch, frame);
        if let Some(part) = before {
            sent[ch].push((current - 1, part));
        }
        if let Some(part) = after {
            sent[ch].push((current, part));
        }
    }

    #[test]
    fn every_channel_is_cut_at_one_sample() {
        let mut c = Cutter::new(2);
        let mut sent = [Vec::new(), Vec::new()];
        // The mic ahead of the call: 1000 and 700.
        feed(&mut c, &mut sent, 0, 0, 1000);
        feed(&mut c, &mut sent, 0, 1, 700);
        assert_eq!(c.cut(), 1000);
        assert!(c.cutting());
        // The mic goes on after the cut; the call reaches it in a frame
        // that straddles it (900..1100), then goes on.
        feed(&mut c, &mut sent, 1, 0, 100);
        feed(&mut c, &mut sent, 1, 1, 200);
        assert!(c.cutting());
        feed(&mut c, &mut sent, 1, 1, 200);
        assert!(!c.cutting());
        feed(&mut c, &mut sent, 1, 1, 50);
        for (ch, frames) in sent.iter().enumerate() {
            // Nothing lost, nothing twice, in order.
            let all: Vec<i16> = frames.iter().flat_map(|f| f.1.clone()).collect();
            let expected: Vec<i16> = (0..all.len() as i16).collect();
            assert_eq!(all, expected, "channel {ch}");
            // Up to the cut to the first meeting, from it to the second.
            let first: usize = frames.iter().filter(|f| f.0 == 0).map(|f| f.1.len()).sum();
            assert_eq!(first, 1000, "channel {ch}");
            assert!(frames.iter().skip_while(|f| f.0 == 0).all(|f| f.0 == 1));
        }
        // The straddling frame was split at the cut.
        assert_eq!(sent[1][2], (0, (900..1000).collect()));
        assert_eq!(sent[1][3], (1, (1000..1100).collect()));
        // Without a cut waiting, everything goes to the meeting recording.
        assert_eq!(c.route(0, vec![1, 2]), (None, Some(vec![1, 2])));
    }

    #[test]
    fn a_cut_where_every_channel_is_waits_for_none() {
        let mut c = Cutter::new(2);
        c.route(0, vec![0; 160]);
        c.route(1, vec![0; 160]);
        assert_eq!(c.cut(), 160);
        assert!(!c.cutting());
        assert_eq!(c.route(1, vec![5]), (None, Some(vec![5])));
    }

    const LIMITS: Limits = Limits {
        pause: Some(PAUSE),
        max_length: Some(MAX_LENGTH),
    };

    #[test]
    fn a_pause_closes_a_meeting_only_while_the_live_transcript_runs() {
        let mut c = Closer::new(LIMITS);
        c.connected(true, 2.0);
        assert!(!c.heard(30.0, 31.0));
        assert!(!c.due(30.0 + PAUSE - 1.0));
        assert!(c.due(30.0 + PAUSE));
        // A phrase later moves the pause on.
        assert!(!c.heard(600.0, 601.0));
        assert!(!c.due(30.0 + PAUSE));
        assert!(c.due(600.0 + PAUSE));
        // Offline: no pause closes it, however long.
        c.connected(false, 700.0);
        assert!(!c.due(600.0 + 3.0 * PAUSE));
        // Back: the pause counts from then.
        c.connected(true, 5000.0);
        assert!(!c.due(5000.0 + PAUSE - 1.0));
        assert!(c.due(5000.0 + PAUSE));
    }

    #[test]
    fn a_meeting_with_nothing_said_closes_after_a_pause_from_its_start() {
        let mut c = Closer::new(LIMITS);
        c.connected(true, 0.5);
        assert!(!c.due(PAUSE));
        assert!(c.due(PAUSE + 0.5));
        // Never connected: only the length closes it.
        let c = Closer::new(LIMITS);
        assert!(!c.due(MAX_LENGTH));
        assert!(c.due(MAX_LENGTH + GRACE));
    }

    #[test]
    fn a_long_meeting_closes_at_its_next_phrase_or_soon_after() {
        let mut c = Closer::new(LIMITS);
        c.connected(true, 0.0);
        // Talk all along: no pause.
        let mut t = 0.0;
        while t + 20.0 < MAX_LENGTH {
            t += 20.0;
            assert!(!c.heard(t - 1.0, t));
            assert!(!c.due(t));
        }
        // The first phrase heard past the length closes it.
        assert!(c.heard(MAX_LENGTH - 0.5, MAX_LENGTH + 1.0));
        // Continuous noise, no phrase: the grace, then closed anyway.
        let mut c = Closer::new(LIMITS);
        c.connected(true, 0.0);
        c.heard(MAX_LENGTH - 60.0, MAX_LENGTH - 59.0);
        assert!(!c.due(MAX_LENGTH + GRACE - 1.0));
        assert!(c.due(MAX_LENGTH + GRACE));
        // Limits off: never.
        let c = Closer::new(Limits {
            pause: None,
            max_length: None,
        });
        assert!(!c.due(1e9));
    }
}
