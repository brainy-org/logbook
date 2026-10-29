//! A recording through the upload's Opus and back (codec.rs), to compare
//! what the models make of each (scripts/der.py, examples/embed.rs). Prints
//! the sizes and the time taken to encode and decode.
//!
//! cargo run --release -p logbook-engine --example opus -- IN OUT.wav [BITRATE]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use logbook_engine::codec;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (input, output, bitrate) = match &args[..] {
        [input, output] => (input, output, 32_000),
        [input, output, bitrate] => (input, output, bitrate.parse()?),
        _ => bail!("usage: opus IN OUT.wav [BITRATE]"),
    };
    let out = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i", input])
        .args(["-ar", "16000", "-ac", "1", "-f", "s16le", "-"])
        .output()
        .context("ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
    }
    let pcm: Vec<i16> = out
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let seconds = pcm.len() as f64 / 16_000.0;

    let t = Instant::now();
    let (mut encoder, mut opus) = codec::StreamEncoder::with_bitrate(pcm.len() as u64, bitrate)?;
    encoder.push(&pcm, &mut opus)?;
    encoder.finish(&mut opus)?;
    let encoded = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let decoded = codec::decode(&opus)?;
    let decoding = t.elapsed().as_secs_f64();
    if decoded.len() != pcm.len() {
        bail!("{} samples back for {}", decoded.len(), pcm.len());
    }
    eprintln!(
        "{seconds:.0} s: {:.1} MB of PCM, {:.1} MB of Opus; encoded in {encoded:.2} s \
         ({:.0}x real time), decoded in {decoding:.2} s ({:.0}x)",
        pcm.len() as f64 * 2.0 / 1e6,
        opus.len() as f64 / 1e6,
        seconds / encoded,
        seconds / decoding,
    );

    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "f32le",
            "-ar",
            "16000",
            "-ac",
            "1",
        ])
        .args(["-i", "-", "-c:a", "pcm_s16le", output])
        .stdin(Stdio::piped())
        .spawn()
        .context("ffmpeg")?;
    let bytes: Vec<u8> = decoded.iter().flat_map(|s| s.to_le_bytes()).collect();
    ffmpeg.stdin.take().context("stdin")?.write_all(&bytes)?;
    if !ffmpeg.wait()?.success() {
        bail!("ffmpeg could not write {output}");
    }
    Ok(())
}
