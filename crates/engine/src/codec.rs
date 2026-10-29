//! Opus, both ends being ours, in two framings (docs/protocol.md):
//!
//! - **Upload** (`POST /finals`): a channel's recording compressed, 14 MB
//!   an hour instead of 115. "LBO1", the number of samples (u64 LE) and
//!   the encoder's lookahead (u16 LE), then 20 ms packets, each a u16 LE
//!   length and its bytes (`StreamEncoder`, `StreamDecoder`).
//! - **Live** (the WebSocket's binary frames, with `"codec":"opus"`): a
//!   stream with no known total. Each frame is the channel's index, the
//!   number of samples it stands for (u32 LE), then the packets those
//!   samples completed, as above; the lookahead is said once, in `start`
//!   (`LiveEncoder`, `LiveDecoder`).
//!
//! Either way the decoder drops the lookahead and emits exactly the
//! samples the encoder was given, no more (the padding of the last
//! packet) and no fewer (the encoder's tail is flushed at the end): the
//! decoded audio lines up with the original sample for sample, because
//! the channels' clocks are compared (echo, speakers).

use anyhow::{Result, bail};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use crate::protocol::SAMPLE_RATE;

const MAGIC: &[u8; 4] = b"LBO1";
const FRAME: usize = SAMPLE_RATE as usize / 50;
const BITRATE: i32 = 32_000;
/// The largest packet Opus makes, as its documentation recommends.
const MAX_PACKET: usize = 4000;

/// 20 ms packets out of samples in parts of any size: what does not fill
/// a packet waits for the next part.
struct Packets {
    encoder: Encoder,
    lookahead: usize,
    /// Samples short of a frame, for the next push.
    rest: Vec<i16>,
    packet: Box<[u8; MAX_PACKET]>,
}

impl Packets {
    fn new(bitrate: i32) -> Result<Self> {
        let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(bitrate))?;
        let lookahead = encoder.get_lookahead()? as usize;
        Ok(Self {
            encoder,
            lookahead,
            rest: Vec::with_capacity(FRAME),
            packet: Box::new([0u8; MAX_PACKET]),
        })
    }

    /// The packets of `pcm`'s whole frames, appended to `out`.
    fn push(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<()> {
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
    fn finish(&mut self, out: &mut Vec<u8>) -> Result<()> {
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

/// One channel encoded as it is read: `new` writes the header (the
/// number of samples is known before the audio), `push` the packets of
/// each part, `finish` the last ones. A meeting of hours is never whole
/// in memory.
pub struct StreamEncoder {
    packets: Packets,
}

impl StreamEncoder {
    /// The encoder for `samples` samples, and the header to send first.
    pub fn new(samples: u64) -> Result<(Self, Vec<u8>)> {
        Self::with_bitrate(samples, BITRATE)
    }

    /// `new` at `bitrate` bits a second (examples/opus.rs compares them).
    pub fn with_bitrate(samples: u64, bitrate: i32) -> Result<(Self, Vec<u8>)> {
        let packets = Packets::new(bitrate)?;
        let mut header = Vec::with_capacity(14);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&samples.to_le_bytes());
        header.extend_from_slice(&(packets.lookahead as u16).to_le_bytes());
        Ok((Self { packets }, header))
    }

    /// The packets of `pcm`'s whole frames, appended to `out`.
    pub fn push(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<()> {
        self.packets.push(pcm, out)
    }

    /// The last packets (`Packets::finish`).
    pub fn finish(mut self, out: &mut Vec<u8>) -> Result<()> {
        self.packets.finish(out)
    }
}

pub fn encode(pcm: &[i16]) -> Result<Vec<u8>> {
    let (mut encoder, mut out) = StreamEncoder::new(pcm.len() as u64)?;
    encoder.push(pcm, &mut out)?;
    encoder.finish(&mut out)?;
    Ok(out)
}

/// Packets back to samples: the lookahead dropped, and nothing past the
/// samples the encoder was given (the padding of its last packet).
struct Unpacker {
    decoder: Decoder,
    /// Samples the decoder has made, the lookahead included.
    made: u64,
    frame: Box<[f32; FRAME]>,
}

impl Unpacker {
    fn new() -> Result<Self> {
        Ok(Self {
            decoder: Decoder::new(SAMPLE_RATE, Channels::Mono)?,
            made: 0,
            frame: Box::new([0f32; FRAME]),
        })
    }

    /// One packet's samples between the lookahead and `lookahead + total`,
    /// appended to `out`.
    fn packet(
        &mut self,
        packet: &[u8],
        lookahead: u64,
        total: u64,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        let n = self
            .decoder
            .decode_float(packet, &mut self.frame[..], false)? as u64;
        let from = lookahead.saturating_sub(self.made).min(n) as usize;
        let to = (lookahead + total).saturating_sub(self.made).min(n) as usize;
        if from < to {
            out.extend_from_slice(&self.frame[from..to]);
        }
        self.made += n;
        Ok(())
    }

    /// Samples made past the lookahead.
    fn samples(&self, lookahead: u64) -> u64 {
        self.made.saturating_sub(lookahead)
    }
}

/// The length-prefixed packets at the start of `data`, each to `each`;
/// returns how many bytes they took (a cut packet is left).
fn packets(data: &[u8], mut each: impl FnMut(&[u8]) -> Result<()>) -> Result<usize> {
    let mut at = 0;
    while let Some(len) = data.get(at..at + 2) {
        let len = u16::from_le_bytes([len[0], len[1]]) as usize;
        let Some(packet) = data.get(at + 2..at + 2 + len) else {
            break;
        };
        each(packet)?;
        at += 2 + len;
    }
    Ok(at)
}

/// One channel decoded as its bytes arrive, in parts of any size: the
/// samples as floats in [-1, 1], the lookahead dropped and cut at the
/// header's number of samples.
pub struct StreamDecoder {
    unpacker: Unpacker,
    /// Bytes of a header or a packet not complete yet.
    pending: Vec<u8>,
    /// From the header, once read: (samples, lookahead).
    header: Option<(u64, u64)>,
}

impl StreamDecoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            unpacker: Unpacker::new()?,
            pending: Vec::new(),
            header: None,
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
        let unpacker = &mut self.unpacker;
        at += packets(&self.pending[at..], |packet| {
            unpacker.packet(packet, lookahead, samples, out)
        })?;
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
        let made = self.unpacker.samples(lookahead);
        if made < samples {
            bail!("the recording holds {made} samples, its header says {samples}");
        }
        Ok(())
    }
}

/// The bytes of a live frame before its packets: the channel (u8) and the
/// number of samples it stands for (u32 LE).
pub const LIVE_HEADER: usize = 5;

/// A live channel's encoder, one per channel and per session: each part
/// of audio becomes one binary frame (`frame`), the last one flushes the
/// encoder (`finish`). A part shorter than a packet, or a remainder past
/// the last whole one, waits in the encoder for the next part: the frame
/// still counts its samples, and the decoder emits them once a later
/// frame brings their packet.
pub struct LiveEncoder {
    packets: Packets,
}

impl LiveEncoder {
    pub fn new() -> Result<Self> {
        Self::with_bitrate(BITRATE)
    }

    pub fn with_bitrate(bitrate: i32) -> Result<Self> {
        Ok(Self {
            packets: Packets::new(bitrate)?,
        })
    }

    /// Samples of delay in the encoder: the decoder drops as many at the
    /// start of the stream. Said in `start`.
    pub fn lookahead(&self) -> u16 {
        self.packets.lookahead as u16
    }

    /// The binary frame of `pcm` on `channel`.
    pub fn frame(&mut self, channel: u8, pcm: &[i16]) -> Result<Vec<u8>> {
        // 100 ms at 32 kbit/s is about 400 bytes; the header and lengths on top.
        let mut out = Vec::with_capacity(LIVE_HEADER + 16 + pcm.len() / 3);
        out.push(channel);
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        self.packets.push(pcm, &mut out)?;
        Ok(out)
    }

    /// The channel's last frame, before `stop`: no new samples, the
    /// packets that carry the last ones out of the encoder.
    pub fn finish(&mut self, channel: u8) -> Result<Vec<u8>> {
        let mut out = vec![channel, 0, 0, 0, 0];
        self.packets.finish(&mut out)?;
        Ok(out)
    }
}

/// A live channel's decoder, one per channel and per session: each
/// frame's packets back to samples, exactly as many in all as the frames
/// said, once the encoder's lookahead is dropped.
pub struct LiveDecoder {
    unpacker: Unpacker,
    lookahead: u64,
    /// Samples the frames so far stand for.
    said: u64,
}

impl LiveDecoder {
    pub fn new(lookahead: u16) -> Result<Self> {
        Ok(Self {
            unpacker: Unpacker::new()?,
            lookahead: lookahead as u64,
            said: 0,
        })
    }

    /// A frame without its channel byte (the number of samples, then the
    /// packets): the samples it completes, appended to `out`.
    pub fn push(&mut self, frame: &[u8], out: &mut Vec<f32>) -> Result<()> {
        let Some((count, data)) = frame.split_first_chunk::<4>() else {
            bail!("a frame without its number of samples");
        };
        self.said += u32::from_le_bytes(*count) as u64;
        let (lookahead, said) = (self.lookahead, self.said);
        let unpacker = &mut self.unpacker;
        let taken = packets(data, |packet| unpacker.packet(packet, lookahead, said, out))?;
        if taken != data.len() {
            bail!("a cut packet");
        }
        Ok(())
    }

    /// Samples the frames said and the packets have not brought yet: 0
    /// once the channel's last frame (`LiveEncoder::finish`) is in.
    pub fn owed(&self) -> u64 {
        self.said
            .saturating_sub(self.unpacker.samples(self.lookahead))
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

    /// A tone that starts `at` samples in, `n` samples in all.
    fn tone(n: usize, at: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                if i < at {
                    0
                } else {
                    let t = i as f32 / SAMPLE_RATE as f32;
                    ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
                }
            })
            .collect()
    }

    /// `pcm` sent live in parts of `sizes` (round and round), through the
    /// frames and back: what each frame's decoding gave, then the tail's.
    fn live(pcm: &[i16], sizes: &[usize]) -> (Vec<Vec<f32>>, Vec<f32>, LiveDecoder) {
        let mut encoder = LiveEncoder::new().unwrap();
        let mut decoder = LiveDecoder::new(encoder.lookahead()).unwrap();
        let mut parts = Vec::new();
        let mut at = 0;
        for &size in sizes.iter().cycle() {
            if at == pcm.len() {
                break;
            }
            let to = (at + size).min(pcm.len());
            let frame = encoder.frame(3, &pcm[at..to]).unwrap();
            assert_eq!(frame[0], 3);
            let mut out = Vec::new();
            decoder.push(&frame[1..], &mut out).unwrap();
            parts.push(out);
            at = to;
        }
        let last = encoder.finish(3).unwrap();
        let mut tail = Vec::new();
        decoder.push(&last[1..], &mut tail).unwrap();
        (parts, tail, decoder)
    }

    #[test]
    fn a_live_stream_comes_back_sample_for_sample() {
        // Parts of uneven sizes: 100 ms, a gap filled by the Timeline,
        // a few samples, an empty one, a second of the replay.
        let n = 41_017;
        let pcm = tone(n, 8000);
        let (parts, tail, decoder) = live(&pcm, &[1600, 2357, 7, 0, 16_000, 319, 321]);
        assert_eq!(decoder.owed(), 0);
        let out: Vec<f32> = parts.iter().flatten().chain(&tail).copied().collect();
        assert_eq!(out.len(), n, "exactly the samples sent");
        // The same as the upload's decoding of the same samples: the
        // lookahead dropped at the start, the tail flushed at the end.
        assert_eq!(out, decode(&encode(&pcm).unwrap()).unwrap());
        // No shift in time: silent before the tone, loud after it.
        let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32;
        assert!(energy(&out[..7800]) < 1e-5);
        assert!(energy(&out[8200..]) > 0.05);
    }

    #[test]
    fn a_live_frame_holds_back_only_the_lookahead() {
        let pcm = tone(16_000, 0);
        let (parts, tail, _) = live(&pcm, &[1600]);
        let lookahead = LiveEncoder::new().unwrap().lookahead() as usize;
        assert!(lookahead > 0 && lookahead < FRAME, "{lookahead}");
        // Frames of whole packets: each gives its samples but the last
        // lookahead's, which come with the next frame.
        assert_eq!(parts[0].len(), 1600 - lookahead);
        for part in &parts[1..] {
            assert_eq!(part.len(), 1600);
        }
        assert_eq!(tail.len(), lookahead);
        // A part shorter than a packet waits in the encoder.
        let (parts, tail, _) = live(&pcm[..100], &[100]);
        assert!(parts[0].is_empty());
        assert_eq!(tail.len(), 100);
    }

    #[test]
    fn a_cut_live_frame_is_refused() {
        let mut encoder = LiveEncoder::new().unwrap();
        let frame = encoder.frame(0, &tone(1600, 0)).unwrap();
        let mut decoder = LiveDecoder::new(encoder.lookahead()).unwrap();
        assert!(
            decoder
                .push(&frame[1..frame.len() - 1], &mut Vec::new())
                .is_err()
        );
        assert!(decoder.push(&[1, 0], &mut Vec::new()).is_err());
    }
}
