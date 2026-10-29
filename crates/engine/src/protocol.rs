//! What the client and the server say to each other (docs/protocol.md):
//! over the WebSocket of a live session, and over HTTP for final passes.
//!
//! WebSocket text frames are JSON messages. Binary frames are audio: one byte, the
//! index of the channel in `Start.channels`, then 16 kHz mono PCM as signed
//! 16-bit little-endian samples.

use serde::{Deserialize, Serialize};

pub use crate::parakeet::Word;

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
        /// Let the server's model clean the final transcript up, if it has
        /// one. Off, the default, keeps the meeting's text away from it.
        #[serde(default)]
        polish: bool,
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
    },
    /// No more audio: the server sends what it still holds, then `Done`.
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready,
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
    /// As in `Start`.
    #[serde(default)]
    pub polish: bool,
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
}

/// `POST /reports` answers with the report as Markdown.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub markdown: String,
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
            matches!(start, ClientMessage::Start { channels, final_pass: false, polish: false, offset: 0.0, .. } if channels[0].name == "mic")
        );
    }
}
