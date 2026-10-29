//! A channel's recording compressed for upload (docs/protocol.md, "Final
//! passes over HTTP"): Opus at 32 kbit/s, 14 MB an hour instead of 115.
//!
//! The format is ours, both ends being ours: "LBO1", the number of samples
//! (u64 LE) and the encoder's lookahead (u16 LE), then 20 ms packets, each
//! a u16 LE length and its bytes. The decoder drops the lookahead and cuts
//! at the number of samples, so that the decoded audio lines up with the
//! original sample for sample: the channels' clocks are compared.

use anyhow::{Result, bail};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use crate::parakeet::SAMPLE_RATE;

const MAGIC: &[u8; 4] = b"LBO1";
const FRAME: usize = SAMPLE_RATE as usize / 50;
const BITRATE: i32 = 32_000;
/// The largest packet Opus makes, as its documentation recommends.
const MAX_PACKET: usize = 4000;

/// One channel encoded as it is read: `new` writes the header (the
/// number of samples is known before the audio), `push` the packets of
/// each part, `finish` the last ones. A meeting of hours is never whole
/// in memory.
pub struct StreamEncoder {
    encoder: Encoder,
    lookahead: usize,
    /// Samples short of a frame, for the next push.
    rest: Vec<i16>,
    packet: Box<[u8; MAX_PACKET]>,
}

impl StreamEncoder {
    /// The encoder for `samples` samples, and the header to send first.
    pub fn new(samples: u64) -> Result<(Self, Vec<u8>)> {
        Self::with_bitrate(samples, BITRATE)
    }

    /// `new` at `bitrate` bits a second (examples/opus.rs compares them).
    pub fn with_bitrate(samples: u64, bitrate: i32) -> Result<(Self, Vec<u8>)> {
        let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(bitrate))?;
        let lookahead = encoder.get_lookahead()? as usize;
        let mut header = Vec::with_capacity(14);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&samples.to_le_bytes());
        header.extend_from_slice(&(lookahead as u16).to_le_bytes());
        let encoder = Self {
            encoder,
            lookahead,
            rest: Vec::with_capacity(FRAME),
            packet: Box::new([0u8; MAX_PACKET]),
        };
        Ok((encoder, header))
    }

    /// The packets of `pcm`'s whole frames, appended to `out`.
    pub fn push(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<()> {
        let mut pcm = pcm;
        if !self.rest.is_empty() {
            let take = (FRAME - self.rest.len()).min(pcm.len());
            self.rest.extend_from_slice(&pcm[..take]);
            pcm = &pcm[take..];
            if self.rest.len() < FRAME {
                return Ok(());
            }
            let frame = std::mem::take(&mut self.rest);
            self.frame(&frame, out)?;
            self.rest = frame;
            self.rest.clear();
        }
        let mut frames = pcm.chunks_exact(FRAME);
        for frame in &mut frames {
            self.frame(frame, out)?;
        }
        self.rest.extend_from_slice(frames.remainder());
        Ok(())
    }

    /// The last packets: the lookahead's worth of silence, so that the
    /// last samples come out of the encoder, and the last frame padded
    /// to a whole one.
    pub fn finish(mut self, out: &mut Vec<u8>) -> Result<()> {
        let mut tail = std::mem::take(&mut self.rest);
        tail.resize(tail.len() + self.lookahead, 0);
        tail.resize(tail.len().div_ceil(FRAME) * FRAME, 0);
        for frame in tail.chunks_exact(FRAME) {
            self.frame(frame, out)?;
        }
        Ok(())
    }

    fn frame(&mut self, frame: &[i16], out: &mut Vec<u8>) -> Result<()> {
        let n = self.encoder.encode(frame, &mut self.packet[..])?;
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.packet[..n]);
        Ok(())
    }
}

pub fn encode(pcm: &[i16]) -> Result<Vec<u8>> {
    let (mut encoder, mut out) = StreamEncoder::new(pcm.len() as u64)?;
    encoder.push(pcm, &mut out)?;
    encoder.finish(&mut out)?;
    Ok(out)
}

/// One channel decoded as its bytes arrive, in parts of any size: the
/// samples as floats in [-1, 1], the lookahead dropped and cut at the
/// header's number of samples.
pub struct StreamDecoder {
    decoder: Decoder,
    /// Bytes of a header or a packet not complete yet.
    pending: Vec<u8>,
    /// From the header, once read: (samples, lookahead).
    header: Option<(u64, u64)>,
    /// Samples the decoder has made, the lookahead included.
    made: u64,
    frame: Box<[f32; FRAME]>,
}

impl StreamDecoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            decoder: Decoder::new(SAMPLE_RATE, Channels::Mono)?,
            pending: Vec::new(),
            header: None,
            made: 0,
            frame: Box::new([0f32; FRAME]),
        })
    }

    /// The samples `data` completes, appended to `out`.
    pub fn push(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(data);
        let mut at = 0;
        if self.header.is_none() {
            if self.pending.len() < 14 {
                return Ok(());
            }
            if &self.pending[..4] != MAGIC {
                bail!("not a recording: wrong magic");
            }
            let samples = u64::from_le_bytes(self.pending[4..12].try_into().unwrap());
            let lookahead = u16::from_le_bytes([self.pending[12], self.pending[13]]) as u64;
            self.header = Some((samples, lookahead));
            at = 14;
        }
        let (samples, lookahead) = self.header.expect("read above");
        while let Some(len) = self.pending.get(at..at + 2) {
            let len = u16::from_le_bytes([len[0], len[1]]) as usize;
            let Some(packet) = self.pending.get(at + 2..at + 2 + len) else {
                break;
            };
            let n = self
                .decoder
                .decode_float(packet, &mut self.frame[..], false)?;
            // The part of this frame past the lookahead and within the samples.
            let from = lookahead.saturating_sub(self.made).min(n as u64) as usize;
            let to = (lookahead + samples)
                .saturating_sub(self.made)
                .min(n as u64) as usize;
            if from < to {
                out.extend_from_slice(&self.frame[from..to]);
            }
            self.made += n as u64;
            at += 2 + len;
        }
        self.pending.drain(..at);
        Ok(())
    }

    /// The whole recording was read: an error if it was cut short.
    pub fn finish(self) -> Result<()> {
        let Some((samples, lookahead)) = self.header else {
            bail!("not a recording: too short");
        };
        if !self.pending.is_empty() {
            bail!("a cut packet");
        }
        if self.made < lookahead + samples {
            bail!(
                "the recording holds {} samples, its header says {samples}",
                self.made.saturating_sub(lookahead)
            );
        }
        Ok(())
    }
}

/// The samples as floats in [-1, 1].
pub fn decode(data: &[u8]) -> Result<Vec<f32>> {
    let mut decoder = StreamDecoder::new()?;
    let mut out = Vec::new();
    decoder.push(data, &mut out)?;
    decoder.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_audio_lines_up_with_the_original() {
        // A 440 Hz tone that starts 0.5 s in, 1.234 s in all.
        let n = 19_744;
        let pcm: Vec<i16> = (0..n)
            .map(|i| {
                if i < 8000 {
                    0
                } else {
                    let t = i as f32 / SAMPLE_RATE as f32;
                    ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
                }
            })
            .collect();
        let data = encode(&pcm).unwrap();
        assert!(data.len() < n * 2 / 4, "{} bytes", data.len());
        let out = decode(&data).unwrap();
        assert_eq!(out.len(), n);
        // Silent before the tone, loud after it: no shift in time.
        let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32;
        assert!(energy(&out[..7800]) < 1e-5);
        assert!(energy(&out[8200..]) > 0.05);
        assert!(decode(b"nope").is_err());

        // Encoded and decoded in parts of odd sizes: the same samples.
        let (mut encoder, mut parts) = StreamEncoder::new(n as u64).unwrap();
        for chunk in pcm.chunks(997) {
            encoder.push(chunk, &mut parts).unwrap();
        }
        encoder.finish(&mut parts).unwrap();
        assert_eq!(parts, data);
        let mut decoder = StreamDecoder::new().unwrap();
        let mut streamed = Vec::new();
        for chunk in data.chunks(333) {
            decoder.push(chunk, &mut streamed).unwrap();
        }
        decoder.finish().unwrap();
        assert_eq!(streamed, out);
        let mut cut = StreamDecoder::new().unwrap();
        cut.push(&data[..data.len() / 2], &mut Vec::new()).unwrap();
        assert!(cut.finish().is_err());
    }
}
