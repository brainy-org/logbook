//! A channel's recording compressed for upload (docs/protocol.md, "Final
//! passes over HTTP"): Opus at 32 kbit/s, 14 MB an hour instead of 115.
//!
//! The format is ours, both ends being ours: "LBO1", the number of samples
//! (u64 LE) and the encoder's lookahead (u16 LE), then 20 ms packets, each
//! a u16 LE length and its bytes. The decoder drops the lookahead and cuts
//! at the number of samples, so that the decoded audio lines up with the
//! original sample for sample: the channels' clocks are compared.

use anyhow::{Context, Result, bail};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use crate::parakeet::SAMPLE_RATE;

const MAGIC: &[u8; 4] = b"LBO1";
const FRAME: usize = SAMPLE_RATE as usize / 50;
const BITRATE: i32 = 32_000;
/// The largest packet Opus makes, as its documentation recommends.
const MAX_PACKET: usize = 4000;

pub fn encode(pcm: &[i16]) -> Result<Vec<u8>> {
    let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
    encoder.set_bitrate(Bitrate::Bits(BITRATE))?;
    let lookahead = encoder.get_lookahead()? as usize;
    let mut out = Vec::with_capacity(pcm.len() / 8 + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(pcm.len() as u64).to_le_bytes());
    out.extend_from_slice(&(lookahead as u16).to_le_bytes());
    // The lookahead's worth of silence at the end, so that the last samples
    // come out of the encoder; the last frame padded to a whole one.
    let mut padded = pcm.to_vec();
    padded.resize(pcm.len() + lookahead, 0);
    padded.resize(padded.len().div_ceil(FRAME) * FRAME, 0);
    let mut packet = [0u8; MAX_PACKET];
    for frame in padded.chunks_exact(FRAME) {
        let n = encoder.encode(frame, &mut packet)?;
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&packet[..n]);
    }
    Ok(out)
}

/// The samples as floats in [-1, 1].
pub fn decode(data: &[u8]) -> Result<Vec<f32>> {
    let header = data.get(..14).context("not a recording: too short")?;
    if &header[..4] != MAGIC {
        bail!("not a recording: wrong magic");
    }
    let samples = u64::from_le_bytes(header[4..12].try_into().unwrap()) as usize;
    let lookahead = u16::from_le_bytes(header[12..14].try_into().unwrap()) as usize;
    let mut decoder = Decoder::new(SAMPLE_RATE, Channels::Mono)?;
    let mut out = Vec::with_capacity(samples + lookahead + FRAME);
    let mut frame = [0f32; FRAME];
    let mut rest = &data[14..];
    while !rest.is_empty() {
        let (len, tail) = rest.split_at_checked(2).context("a cut packet")?;
        let len = u16::from_le_bytes([len[0], len[1]]) as usize;
        let (packet, tail) = tail.split_at_checked(len).context("a cut packet")?;
        let n = decoder.decode_float(packet, &mut frame, false)?;
        out.extend_from_slice(&frame[..n]);
        rest = tail;
    }
    if out.len() < lookahead + samples {
        bail!(
            "the recording holds {} samples, its header says {samples}",
            out.len().saturating_sub(lookahead)
        );
    }
    out.truncate(lookahead + samples);
    out.drain(..lookahead);
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
    }
}
