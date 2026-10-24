//! What the client and the server say to each other over the WebSocket
//! (docs/protocol.md).
//!
//! Text frames are JSON messages. Binary frames are audio: one byte, the
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Start {
        channels: Vec<ChannelConfig>,
    },
    /// No more audio: the server sends what it still holds, then `Done`.
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready,
    Utterance(Utterance),
    Done,
    Error { message: String },
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
        assert!(matches!(start, ClientMessage::Start { channels } if channels[0].name == "mic"));
    }
}
