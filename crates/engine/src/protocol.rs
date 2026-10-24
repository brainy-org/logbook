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
    },
    /// No more audio: the server sends what it still holds, then `Done`.
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready,
    Utterance(Utterance),
    /// The live transcript is complete; the final pass has started.
    Finalizing {
        /// Seconds of the session's recording.
        audio_s: f64,
        /// The pass's id: if the connection is lost, its result is at
        /// `GET /finals/{id}` for a while.
        id: String,
    },
    /// The whole meeting read again: every channel's segments by start.
    Final {
        utterances: Vec<Utterance>,
        warnings: Vec<String>,
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
        utterances: Vec<Utterance>,
        warnings: Vec<String>,
    },
    Failed {
        message: String,
    },
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
            matches!(start, ClientMessage::Start { channels, final_pass: false } if channels[0].name == "mic")
        );
    }
}
