//! `series::meetings` end to end, without models: a fake logbook-server on
//! 127.0.0.1 speaks the WebSocket protocol (docs/protocol.md) and makes
//! its "transcript" from the samples themselves, so that the test decides
//! where speech is. A recording with speech, a pause and speech again
//! must become two meetings cut at one sample on every channel, each
//! session given exactly its samples, and each meeting's notes must be
//! the ones typed in it, however late they arrive.
//!
//! The fake server: `start` → `ready`; each binary frame is kept, per
//! connection and channel; every frame of the first channel ("mic") that
//! holds speech (a positive sample: silence is negative here, so that
//! every sample still carries its position) is answered at once with a
//! live `utterance` covering that frame, on the session's clock; on
//! `stop`, once the test lets it (`finals`), `final` with those
//! utterances, then `done`. Utterances are counted as they are sent, so
//! the feeder waits for each to reach the series before the next frame:
//! the `Closer` sees speech and silence exactly where the samples say,
//! whatever the machine's load.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use logbook_client::history;
use logbook_client::meeting::{Backend, Outcome, Publish};
use logbook_client::remote::{self, Server};
use logbook_client::series::{self, Limits, Notebook, Series};
use logbook_engine::protocol::{ChannelConfig, ClientMessage, ServerMessage, Utterance};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

const RATE: u64 = 16_000;
/// 100 ms, as the capture sends.
const FRAME: u64 = 1_600;
const PAUSE: f64 = 1.0;

/// Sample `p` of `channel` on the recording's clock: its magnitude says
/// where it is (nothing lost, nothing twice, in order), its sign whether
/// it is speech.
fn sample(channel: usize, p: u64, speech: bool) -> i16 {
    let magnitude = 1 + ((p * (channel as u64 + 1)) % 30_000) as i16;
    if speech { magnitude } else { -magnitude }
}

/// What a session was sent: every channel's samples, and its frames'
/// lengths.
#[derive(Default)]
struct Session {
    samples: [Vec<i16>; 2],
    frames: [Vec<usize>; 2],
}

/// What the fake server heard and said.
#[derive(Default)]
struct Heard {
    /// Per connection, in the order they came.
    sessions: Mutex<Vec<Session>>,
    /// Samples received on every connection, counted after the utterance
    /// they make is sent.
    received: AtomicU64,
    utterances: AtomicUsize,
}

async fn fake_server(listener: TcpListener, heard: Arc<Heard>, finals: watch::Receiver<bool>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (heard, mut finals) = (heard.clone(), finals.clone());
        tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(start))) = socket.next().await else {
                panic!("no start");
            };
            let ClientMessage::Start {
                channels,
                final_pass,
                offset,
                ..
            } = serde_json::from_str(start.as_str()).unwrap()
            else {
                panic!("not a start: {start}");
            };
            assert_eq!(channels.len(), 2);
            assert!(final_pass, "a kept meeting asks for the final pass");
            assert_eq!(offset, 0.0, "no session is taken up again here");
            let index = {
                let mut sessions = heard.sessions.lock().unwrap();
                sessions.push(Default::default());
                sessions.len() - 1
            };
            let send = |m: &ServerMessage| Message::Text(serde_json::to_string(m).unwrap().into());
            socket
                .send(send(&ServerMessage::Ready { codec: None }))
                .await
                .unwrap();
            let mut said = Vec::new();
            while let Some(message) = socket.next().await {
                match message.unwrap() {
                    Message::Binary(frame) => {
                        let (&channel, pcm) = frame.split_first().unwrap();
                        let samples: Vec<i16> = pcm
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]]))
                            .collect();
                        let n = samples.len() as u64;
                        let at = {
                            let mut sessions = heard.sessions.lock().unwrap();
                            let session = &mut sessions[index];
                            session.frames[channel as usize].push(samples.len());
                            let kept = &mut session.samples[channel as usize];
                            kept.extend_from_slice(&samples);
                            kept.len() as u64 - n
                        };
                        if channel == 0 && samples.iter().any(|&s| s > 0) {
                            let u = Utterance {
                                channel: "mic".into(),
                                speaker: None,
                                start: at as f64 / RATE as f64,
                                end: (at + n) as f64 / RATE as f64,
                                text: format!("words at {at}"),
                                words: Vec::new(),
                            };
                            said.push(u.clone());
                            socket
                                .send(send(&ServerMessage::Utterance(u)))
                                .await
                                .unwrap();
                            heard.utterances.fetch_add(1, Ordering::SeqCst);
                        }
                        heard.received.fetch_add(n, Ordering::SeqCst);
                    }
                    Message::Text(text) => {
                        let ClientMessage::Stop = serde_json::from_str(text.as_str()).unwrap()
                        else {
                            panic!("not a stop: {text}");
                        };
                        // The final pass takes as long as the test wants.
                        finals.wait_for(|&open| open).await.unwrap();
                        let last = ServerMessage::Final {
                            utterances: std::mem::take(&mut said),
                            warnings: Vec::new(),
                            voices: Vec::new(),
                        };
                        socket.send(send(&last)).await.unwrap();
                        socket.send(send(&ServerMessage::Done)).await.unwrap();
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
    }
}

/// What the series told its caller.
#[derive(Default)]
struct Told {
    utterances: usize,
    started: Vec<(usize, f64)>,
    ended: Vec<(usize, Result<Option<Outcome>, String>)>,
}

struct Events(Arc<Mutex<Told>>);

impl remote::Events for Events {
    fn utterance(&mut self, _u: Utterance) {
        self.0.lock().unwrap().utterances += 1;
    }
}

impl Series for Events {
    fn meeting_started(&mut self, n: usize, at: f64) {
        self.0.lock().unwrap().started.push((n, at));
    }

    fn meeting_ended(&mut self, n: usize, outcome: &anyhow::Result<Option<Outcome>>) {
        let outcome = match outcome {
            Ok(o) => Ok(o.clone()),
            Err(e) => Err(format!("{e:#}")),
        };
        self.0.lock().unwrap().ended.push((n, outcome));
    }
}

/// Waits for `done`, briefly, or fails the test.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// The capture: frames of every channel on one clock, the second channel
/// half a frame out of step with the first, so that a cut where the first
/// is falls inside one of the second's frames.
struct Capture {
    tx: Option<mpsc::UnboundedSender<(u8, Vec<i16>)>>,
    fed: [Vec<i16>; 2],
    speech: Vec<Range<u64>>,
    heard: Arc<Heard>,
    told: Arc<Mutex<Told>>,
}

impl Capture {
    fn push(&mut self, channel: usize, n: u64) {
        let from = self.fed[channel].len() as u64;
        let frame: Vec<i16> = (from..from + n)
            .map(|p| sample(channel, p, self.speech.iter().any(|r| r.contains(&p))))
            .collect();
        self.fed[channel].extend_from_slice(&frame);
        let tx = self.tx.as_ref().expect("the capture runs");
        tx.send((channel as u8, frame)).unwrap();
    }

    /// 100 ms of every channel, then waits until the server has it all
    /// and every utterance it made has reached the series.
    async fn step(&mut self) {
        self.push(0, FRAME);
        self.push(1, FRAME);
        self.settle().await;
    }

    async fn settle(&self) {
        let fed = (self.fed[0].len() + self.fed[1].len()) as u64;
        let heard = &self.heard;
        until("the server to hear every frame", || {
            heard.received.load(Ordering::SeqCst) == fed
        })
        .await;
        let sent = heard.utterances.load(Ordering::SeqCst);
        until("the utterances to reach the series", || {
            self.told.lock().unwrap().utterances >= sent
        })
        .await;
    }

    /// Where the first channel is, in samples.
    fn position(&self) -> u64 {
        self.fed[0].len() as u64
    }
}

fn data_folder() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("logbook-series-{}-{nanos}", std::process::id()))
}

const NOTES_0: &str = "the last keystrokes of the first meeting, sent after the cut";
const NOTES_1: &str = "notes of the second meeting";
const NOTES_1_END: &str = "notes of the second meeting\nand a line typed before stop";

#[tokio::test]
async fn a_pause_cuts_a_recording_into_meetings_that_keep_their_samples_and_notes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let heard = Arc::new(Heard::default());
    let (open_finals, finals) = watch::channel(false);
    tokio::spawn(fake_server(listener, heard.clone(), finals));

    let backend = Backend::Server(Server::new(&format!("http://{address}")));
    let data = data_folder();
    let channels = vec![
        ChannelConfig {
            name: "mic".into(),
            diarize: false,
            echo_of: None,
        },
        ChannelConfig {
            name: "system".into(),
            diarize: true,
            echo_of: None,
        },
    ];
    // Kept in the history: where a meeting's notes are read back, with
    // neither brainy nor a report.
    let publish = Publish {
        final_pass: true,
        keep: true,
        me: "Me".into(),
        ..Publish::default()
    };
    let limits = Limits {
        pause: Some(PAUSE),
        max_length: None,
    };
    let notes = Notebook::default();
    let told = Arc::new(Mutex::new(Told::default()));
    let mut events = Events(told.clone());
    let (tx, audio) = mpsc::unbounded_channel();
    let mut capture = Capture {
        tx: Some(tx),
        fed: [Vec::new(), Vec::new()],
        speech: Vec::new(),
        heard: heard.clone(),
        told: told.clone(),
    };
    // Speech for 2 s, then silence until the meeting closes.
    capture.speech.push(0..2 * RATE);

    let recording = series::meetings(
        &backend,
        &data,
        channels,
        &publish,
        audio,
        &notes,
        limits,
        &mut events,
    );
    let capturing = async {
        capture.push(1, FRAME / 2);
        capture.settle().await;
        while told.lock().unwrap().started.is_empty() {
            assert!(capture.position() < 10 * RATE, "no meeting closed");
            capture.step().await;
        }
        // The pause counts from the last phrase's end (2 s); the first
        // frame that reaches it closes the meeting, every channel there.
        let cut = 3 * RATE;
        assert_eq!(told.lock().unwrap().started, [(1, 3.0)]);

        // The app's last keystrokes in the first meeting arrive after the
        // cut, while its final pass runs; the second meeting's notes start.
        notes.set(0, NOTES_0.into());
        notes.set(1, NOTES_1.into());
        assert_eq!(notes.get(0), NOTES_0);
        assert_eq!(notes.get(1), NOTES_1);
        open_finals.send(true).unwrap();
        until("the first meeting to end", || {
            !told.lock().unwrap().ended.is_empty()
        })
        .await;
        // A meeting that has ended takes no more notes, and says nothing.
        notes.set(0, "typed in a meeting that has ended".into());
        assert_eq!(notes.get(0), "");
        assert_eq!(notes.get(1), NOTES_1);

        // Half a second of silence, 1.5 s of speech, a little silence:
        // shorter than the pause, the second meeting stays open.
        let from = capture.position() + RATE / 2;
        capture.speech.push(from..from + 3 * RATE / 2);
        while capture.position() < from + 3 * RATE / 2 + RATE / 5 {
            capture.step().await;
        }
        // The app's stop: the notes' last state, then the audio's end.
        notes.set(1, NOTES_1_END.into());
        capture.tx = None;
        cut
    };
    let (last, cut) = tokio::join!(recording, capturing);
    let last = last.unwrap().expect("a kept meeting has an outcome");

    let told = told.lock().unwrap();
    assert_eq!(told.started, [(1, 3.0)], "one cut only");
    assert_eq!(told.ended.len(), 1);
    let (n, first) = &told.ended[0];
    assert_eq!(*n, 0);
    let first = first.as_ref().unwrap().as_ref().unwrap();
    assert!(!first.silent && !last.silent);
    assert_ne!(first.id, last.id);

    // Each session had exactly its meeting's samples, cut at one sample
    // on every channel: nothing lost, nothing twice.
    let sessions = heard.sessions.lock().unwrap();
    assert_eq!(sessions.len(), 2);
    for ch in 0..2 {
        let fed = &capture.fed[ch];
        assert_eq!(sessions[0].samples[ch].len() as u64, cut, "channel {ch}");
        assert!(
            sessions[0].samples[ch] == fed[..cut as usize],
            "channel {ch}"
        );
        assert!(
            sessions[1].samples[ch] == fed[cut as usize..],
            "channel {ch}"
        );
    }
    // The call's frame across the cut was split in two halves.
    let half = (FRAME / 2) as usize;
    assert_eq!(sessions[0].frames[1].last(), Some(&half));
    assert_eq!(sessions[1].frames[1].first(), Some(&half));
    assert_eq!(sessions[1].frames[0].first(), Some(&(FRAME as usize)));

    // Each meeting's notes are the ones typed in it.
    let kept_first = history::load(&data, &first.id).unwrap();
    let kept_last = history::load(&data, &last.id).unwrap();
    assert_eq!(kept_first.notes, NOTES_0);
    assert_eq!(kept_last.notes, NOTES_1_END);
    // Its transcript is its own: 2 s of speech in 100 ms frames.
    assert_eq!(kept_first.utterances.len(), 20);
    assert!(!kept_last.utterances.is_empty());
    // On its own clock: its speech starts 0.5 s after the cut.
    assert_eq!(kept_last.utterances[0].start, 0.5);
    // Both published: nothing left pending.
    let pending = std::fs::read_dir(data.join("pending")).unwrap().count();
    assert_eq!(pending, 0);
    let _ = std::fs::remove_dir_all(&data);
}
