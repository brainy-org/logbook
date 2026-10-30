//! What the client and the server say to each other (docs/protocol.md):
//! over the WebSocket of a live session, and over HTTP for final passes.
//!
//! WebSocket text frames are JSON messages. Binary frames are audio: one byte, the
//! index of the channel in `Start.channels`, then in the codec `start` and
//! `ready` agreed on: 16 kHz mono PCM as signed 16-bit little-endian
//! samples, or Opus (codec.rs, `LiveEncoder`).

use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};

use crate::codec::LiveDecoder;

/// The audio's rate, everywhere: the models', the wire's, the recordings'.
pub const SAMPLE_RATE: u32 = 16_000;

/// A word with its times, as libparakeet returns it (`parakeet.rs`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Word {
    pub w: String,
    pub start: f64,
    pub end: f64,
    pub conf: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// "mic", "system": shown with every utterance.
    pub name: String,
    /// Tell the speakers apart. Off for a channel that is one person (your
    /// own microphone with headphones), on for the call's audio or a room.
    pub diarize: bool,
    /// The channel this one may hear again: "system" for a microphone near
    /// the speakers. Its words that the other channel said at the same time
    /// are dropped as echo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo_of: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Start {
        channels: Vec<ChannelConfig>,
        /// After `stop`, read the whole recording again and send `final`.
        /// The server keeps the audio for it, and deletes it afterwards.
        #[serde(default)]
        final_pass: bool,
        /// Where on the meeting's clock this session's first sample is, in
        /// seconds: every time the server sends is on the meeting's clock.
        /// A session that takes a meeting's final pass up again starts
        /// where the last one's final segments stopped.
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f64,
        /// The meeting's speakers so far, from the last `final_segments`
        /// of the session this one takes up: the final pass keeps their
        /// numbers.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        voices: Vec<SpeakerVoice>,
        /// The codec the client would send the audio in. A server that
        /// takes it says so in `ready`; one that does not (or an older
        /// one, which ignores the field) answers a bare `ready`, and the
        /// audio is PCM.
        #[serde(
            default,
            skip_serializing_if = "Codec::is_pcm",
            deserialize_with = "codec_or_pcm"
        )]
        codec: Codec,
        /// With `codec` Opus: the encoder's lookahead in samples, which
        /// the decoder drops (codec.rs). Opus without it is not taken.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lookahead: Option<u16>,
    },
    /// No more audio: the server sends what it still holds, then `Done`.
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The session exists. `codec`: the one `start` asked for, taken;
    /// absent, the audio is PCM.
    Ready {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codec: Option<Codec>,
    },
    Utterance(Utterance),
    /// Final segments, during the meeting: every channel's segments that
    /// start before `until` (seconds), by start, never sent again. The
    /// final pass follows the meeting a minute or so behind.
    FinalSegments {
        until: f64,
        utterances: Vec<Utterance>,
        /// The voice of every speaker of the diarized channels so far: what
        /// the client puts a name on (cli voices.rs).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        voices: Vec<SpeakerVoice>,
    },
    /// The live transcript is complete; the rest of the final pass runs.
    Finalizing {
        /// Seconds of the session's recording.
        audio_s: f64,
        /// The pass's id: if the connection is lost, its result is at
        /// `GET /finals/{id}` for a while.
        id: String,
    },
    /// The final pass is complete: the segments not yet sent as
    /// `final_segments`, by start.
    Final {
        utterances: Vec<Utterance>,
        warnings: Vec<String>,
        /// As in `FinalSegments`, at the meeting's end.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        voices: Vec<SpeakerVoice>,
    },
    Done,
    Error {
        message: String,
    },
}

/// The meeting part of `POST /finals`: the channels, in the order of the
/// `audio` parts that follow it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Upload {
    pub channels: Vec<ChannelConfig>,
    /// As in `Start`: the audio parts start there on the meeting's clock.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: f64,
    /// As in `Start`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<SpeakerVoice>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// `POST /finals` answers with the id of the pass it started.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Started {
    pub id: String,
}

/// `GET /finals/{id}`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FinalState {
    Running {
        audio_s: f64,
        /// How far the pass of a whole meeting (an upload, or one read
        /// again in the client's process) has got; None for a live
        /// session's pass, and from a server older than it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<PassProgress>,
    },
    Done {
        /// Every segment starting at or after `from`, none before.
        utterances: Vec<Utterance>,
        warnings: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        voices: Vec<SpeakerVoice>,
        /// On the meeting's clock. A live session keeps the last minutes
        /// of what it sent (engine session.rs, `KEEP_SENT`): a client
        /// whose final segments stopped before `from` has a gap, and
        /// uploads its audio from there instead. 0, everything, from a
        /// server older than it.
        #[serde(default)]
        from: f64,
    },
    Failed {
        message: String,
    },
}

/// Where the pass of a whole meeting is: its phase, and `done` of it, from
/// 0 to 1 (engine final_pass.rs).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct PassProgress {
    pub phase: PassPhase,
    pub done: f64,
}

impl PassProgress {
    pub const QUEUED: Self = Self {
        phase: PassPhase::Queued,
        done: 0.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PassPhase {
    /// Behind other passes: the server runs one at a time.
    Queued,
    /// Every channel read once to find its pauses (fast).
    Reading,
    /// Range by range, in order (nearly all of the time).
    Transcribing,
}

/// `POST /reports`: the meeting's report written by the server's model
/// (engine report.rs). The client sends the transcript as Markdown with
/// the speakers' names it gave them: the server never sees its voices.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReportRequest {
    #[serde(default)]
    pub title: String,
    /// What the person wrote during the meeting; may be empty.
    #[serde(default)]
    pub notes: String,
    pub transcript: String,
    /// The meeting's tasks, found first (`POST /tasks`): the report's
    /// action items are these, not a list of its own. None from a client
    /// older than it, or without tasks: the model lists them itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<Task>,
}

/// `POST /reports` answers with the report as Markdown.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub markdown: String,
}

/// `POST /tasks`: the tasks agreed in the meeting, found by the server's
/// model (engine tasks.rs), from the same transcript as the report.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TasksRequest {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: String,
    pub transcript: String,
    /// The day the meeting took place, `YYYY-MM-DD` (the client's local
    /// date): a due date said as "Friday" is resolved from it.
    pub date: String,
}

/// `POST /tasks` answers with the tasks found, in the order they were
/// discussed; none when the model's answer could not be read.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Tasks {
    pub tasks: Vec<Task>,
}

/// A task someone took on or was given in the meeting, as the model
/// proposes it: a person confirms it (docs/decisions/
/// 2026-10-29-tasks-as-collection-rows.md).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Task {
    /// What is to be done, in the meeting's language.
    pub task: String,
    /// The speaker who owns it, exactly as the transcript labels them;
    /// None when it is nobody's, or someone who did not speak.
    #[serde(default)]
    pub owner: Option<String>,
    /// `YYYY-MM-DD`, only when a date was said.
    #[serde(default)]
    pub due: Option<String>,
    /// Seconds from the meeting's start of the turn where it was agreed:
    /// every task has one, so that its row links to that moment.
    pub at: f64,
}

/// A run of words from one speaker of one channel. Times are seconds from the
/// first sample of that channel.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Utterance {
    pub channel: String,
    /// 0-based, in order of first appearance on the channel; None on a channel
    /// that is not diarized, or for words no turn covers.
    pub speaker: Option<u32>,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
}

impl Utterance {
    /// Moved `by` seconds later, with its words.
    pub fn shifted(mut self, by: f64) -> Self {
        if by != 0.0 {
            self.start += by;
            self.end += by;
            for w in &mut self.words {
                w.start += by;
                w.end += by;
            }
        }
        self
    }
}

/// A meeting speaker's voice: the mean of its embeddings (WeSpeaker
/// ResNet34-LM, L2-normalized) over the audio where it spoke alone. A
/// voiceprint: the server does not keep it past the meeting.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpeakerVoice {
    pub channel: String,
    /// As in `Utterance::speaker`.
    pub speaker: u32,
    pub embedding: Vec<f32>,
    /// Seconds of audio behind it: a short voice is a noisy one.
    pub seconds: f64,
}

/// How the binary frames carry the audio (docs/protocol.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// 16 kHz mono s16le: 256 kbit/s a channel. What every server takes.
    #[default]
    Pcm,
    /// Opus at 32 kbit/s, framed for a stream (codec.rs).
    Opus,
}

impl Codec {
    fn is_pcm(&self) -> bool {
        *self == Codec::Pcm
    }

    /// The server's answer to `start`: the codec in `ready`, None for a
    /// bare one (PCM). Opus is taken with its lookahead; a codec this
    /// server does not know was read as PCM.
    pub fn answer(asked: Codec, lookahead: Option<u16>) -> Option<Codec> {
        match (asked, lookahead) {
            (Codec::Opus, Some(_)) => Some(Codec::Opus),
            _ => None,
        }
    }
}

/// A codec named by a newer client that this server does not know is
/// PCM, which `ready` then says nothing about: the session still opens.
fn codec_or_pcm<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Codec, D::Error> {
    let name = String::deserialize(deserializer)?;
    Ok(match name.as_str() {
        "opus" => Codec::Opus,
        _ => Codec::Pcm,
    })
}

/// A session's binary frames read back on the server, in the codec agreed:
/// each channel's samples as floats in [-1, 1], exactly those the client
/// captured.
pub enum AudioIn {
    Pcm,
    /// A decoder per channel, for this session only.
    Opus(Vec<LiveDecoder>),
}

impl AudioIn {
    /// For `channels` channels, in the codec `ready` said (None: PCM).
    pub fn new(codec: Option<Codec>, lookahead: Option<u16>, channels: usize) -> Result<Self> {
        Ok(match codec {
            Some(Codec::Opus) => AudioIn::Opus(
                (0..channels)
                    .map(|_| LiveDecoder::new(lookahead.unwrap_or(0)))
                    .collect::<Result<_>>()?,
            ),
            _ => AudioIn::Pcm,
        })
    }

    /// A binary frame: its channel and the samples it completes (with
    /// Opus, maybe none yet). None for an empty frame or a channel the
    /// session does not have; an error for a frame Opus cannot read.
    pub fn frame(&mut self, frame: &[u8]) -> Result<Option<(usize, Vec<f32>)>> {
        match self {
            AudioIn::Pcm => Ok(decode_audio(frame)),
            AudioIn::Opus(decoders) => {
                let Some((&channel, rest)) = frame.split_first() else {
                    return Ok(None);
                };
                let Some(decoder) = decoders.get_mut(channel as usize) else {
                    return Ok(None);
                };
                let mut pcm = Vec::with_capacity(1600 + 320);
                decoder.push(rest, &mut pcm)?;
                Ok(Some((channel as usize, pcm)))
            }
        }
    }

    /// Samples the frames said and did not bring, all channels together:
    /// 0 after a client's last frames (it flushes its encoders before
    /// `stop`).
    pub fn owed(&self) -> u64 {
        match self {
            AudioIn::Pcm => 0,
            AudioIn::Opus(decoders) => decoders.iter().map(LiveDecoder::owed).sum(),
        }
    }
}

pub fn encode_audio(channel: u8, samples: &[i16]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + samples.len() * 2);
    frame.push(channel);
    for s in samples {
        frame.extend_from_slice(&s.to_le_bytes());
    }
    frame
}

/// The channel and the samples as floats in [-1, 1]; None for an empty frame.
pub fn decode_audio(frame: &[u8]) -> Option<(usize, Vec<f32>)> {
    let (&channel, pcm) = frame.split_first()?;
    let samples = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    Some((channel as usize, samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_pass_reads_with_or_without_its_progress() {
        let old: FinalState =
            serde_json::from_str(r#"{"state":"running","audio_s":12.0}"#).unwrap();
        assert!(matches!(old, FinalState::Running { progress: None, .. }));
        let new = FinalState::Running {
            audio_s: 12.0,
            progress: Some(PassProgress {
                phase: PassPhase::Transcribing,
                done: 0.5,
            }),
        };
        assert_eq!(
            serde_json::to_string(&new).unwrap(),
            r#"{"state":"running","audio_s":12.0,"progress":{"phase":"transcribing","done":0.5}}"#
        );
    }

    #[test]
    fn audio_round_trip() {
        let frame = encode_audio(1, &[0, 16384, -32768]);
        let (channel, samples) = decode_audio(&frame).unwrap();
        assert_eq!(channel, 1);
        assert_eq!(samples, vec![0.0, 0.5, -1.0]);
    }

    #[test]
    fn messages_are_tagged() {
        let json = serde_json::to_string(&ClientMessage::Stop).unwrap();
        assert_eq!(json, r#"{"type":"stop"}"#);
        let start: ClientMessage =
            serde_json::from_str(r#"{"type":"start","channels":[{"name":"mic","diarize":false}]}"#)
                .unwrap();
        assert!(
            matches!(start, ClientMessage::Start { channels, final_pass: false, offset: 0.0, codec: Codec::Pcm, lookahead: None, .. } if channels[0].name == "mic")
        );
    }

    /// The messages of the protocol before the codec, as an older client
    /// or server reads and writes them.
    mod older {
        use super::super::ChannelConfig;
        use serde::{Deserialize, Serialize};

        #[derive(Deserialize, Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum ClientMessage {
            Start {
                channels: Vec<ChannelConfig>,
                #[serde(default)]
                final_pass: bool,
            },
        }

        #[derive(Debug, Deserialize, Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum ServerMessage {
            Ready,
        }
    }

    fn mic() -> Vec<ChannelConfig> {
        vec![ChannelConfig {
            name: "mic".into(),
            diarize: false,
            echo_of: None,
        }]
    }

    #[test]
    fn a_new_client_and_an_older_server_agree_on_pcm() {
        // The new client asks for Opus; the older server reads its start
        // (the fields it does not know ignored) and answers a bare ready,
        // which the new client reads as PCM.
        let asked = serde_json::to_string(&ClientMessage::Start {
            channels: mic(),
            final_pass: true,
            offset: 0.0,
            voices: Vec::new(),
            codec: Codec::Opus,
            lookahead: Some(104),
        })
        .unwrap();
        assert!(
            asked.contains(r#""codec":"opus","lookahead":104"#),
            "{asked}"
        );
        let older::ClientMessage::Start { final_pass, .. } = serde_json::from_str(&asked).unwrap();
        assert!(final_pass);
        let ready = serde_json::to_string(&older::ServerMessage::Ready).unwrap();
        assert_eq!(ready, r#"{"type":"ready"}"#);
        let ready: ServerMessage = serde_json::from_str(&ready).unwrap();
        assert!(matches!(ready, ServerMessage::Ready { codec: None }));
    }

    #[test]
    fn an_older_client_and_a_new_server_agree_on_pcm() {
        // The older client asks nothing: the new server answers a bare
        // ready, which the older client reads, and expects PCM.
        let asked = serde_json::to_string(&older::ClientMessage::Start {
            channels: mic(),
            final_pass: false,
        })
        .unwrap();
        let ClientMessage::Start {
            codec, lookahead, ..
        } = serde_json::from_str(&asked).unwrap()
        else {
            panic!("a start");
        };
        let answer = Codec::answer(codec, lookahead);
        assert_eq!(answer, None);
        let ready = serde_json::to_string(&ServerMessage::Ready { codec: answer }).unwrap();
        assert_eq!(ready, r#"{"type":"ready"}"#);
        let older::ServerMessage::Ready = serde_json::from_str(&ready).unwrap();
        let mut audio = AudioIn::new(answer, lookahead, 1).unwrap();
        let frame = encode_audio(0, &[0, 16384, -32768]);
        let (channel, samples) = audio.frame(&frame).unwrap().unwrap();
        assert_eq!((channel, samples), (0, vec![0.0, 0.5, -1.0]));
    }

    #[test]
    fn a_new_client_and_a_new_server_agree_on_opus() {
        let start: ClientMessage = serde_json::from_str(
            r#"{"type":"start","channels":[{"name":"mic","diarize":false}],"codec":"opus","lookahead":104}"#,
        )
        .unwrap();
        let ClientMessage::Start {
            codec, lookahead, ..
        } = start
        else {
            panic!("a start");
        };
        let answer = Codec::answer(codec, lookahead);
        assert_eq!(answer, Some(Codec::Opus));
        let ready = serde_json::to_string(&ServerMessage::Ready { codec: answer }).unwrap();
        assert_eq!(ready, r#"{"type":"ready","codec":"opus"}"#);
        // Opus without its lookahead, or a codec this server does not
        // know: PCM, and the session still opens.
        assert_eq!(Codec::answer(Codec::Opus, None), None);
        let start: ClientMessage = serde_json::from_str(
            r#"{"type":"start","channels":[{"name":"mic","diarize":false}],"codec":"flac"}"#,
        )
        .unwrap();
        assert!(matches!(
            start,
            ClientMessage::Start {
                codec: Codec::Pcm,
                ..
            }
        ));

        // The frames through `AudioIn`, two channels interleaved: each
        // gets exactly its samples back.
        let mut encoders = [
            crate::codec::LiveEncoder::new().unwrap(),
            crate::codec::LiveEncoder::new().unwrap(),
        ];
        let lookahead = Some(encoders[0].lookahead());
        let mut audio = AudioIn::new(answer, lookahead, 2).unwrap();
        let mut got = [Vec::new(), Vec::new()];
        let mut take = |frame: Vec<u8>, got: &mut [Vec<f32>; 2]| {
            if let Some((channel, pcm)) = audio.frame(&frame).unwrap() {
                got[channel].extend(pcm);
            }
        };
        let sizes = [(0, 1600), (1, 1600), (0, 1000), (1, 3333), (0, 2333)];
        for (channel, n) in sizes {
            let frame = encoders[channel]
                .frame(channel as u8, &vec![100; n])
                .unwrap();
            take(frame, &mut got);
        }
        for (channel, encoder) in encoders.iter_mut().enumerate() {
            take(encoder.finish(channel as u8).unwrap(), &mut got);
        }
        assert_eq!(got[0].len(), 4933);
        assert_eq!(got[1].len(), 4933);
        assert_eq!(audio.owed(), 0);
    }
}
