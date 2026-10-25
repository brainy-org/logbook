//! Compares two speaker encoders on the same clips: the cosine of their
//! embeddings of each clip, and of different clips with each.
//!
//! cargo run --release -p logbook-engine --example embed -- LIB A.gguf B.gguf CLIP.wav...

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use logbook_engine::parakeet::{Kind, Model, Parakeet};

fn load(lib: &std::sync::Arc<Parakeet>, path: &str) -> Result<std::sync::Arc<Model>> {
    Model::load_component(lib, Path::new(path), Some("voice"), Kind::Speaker)
        .or_else(|_| Model::load(lib, Path::new(path), Kind::Speaker))
}

fn decode(path: &str) -> Result<Vec<f32>> {
    let out = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-i",
            path,
            "-ar",
            "16000",
            "-ac",
            "1",
            "-f",
            "s16le",
            "-",
        ])
        .output()
        .context("ffmpeg")?;
    Ok(out
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lib = Parakeet::open(Some(Path::new(&args[0])))?;
    let a = load(&lib, &args[1])?;
    let b = load(&lib, &args[2])?;
    let mut ea = Vec::new();
    let mut eb = Vec::new();
    for clip in &args[3..] {
        let pcm = decode(clip)?;
        ea.push(a.embed(&pcm)?);
        eb.push(b.embed(&pcm)?);
        let n = ea.len() - 1;
        println!(
            "{clip}: dim {} / {}, cos(A, B) {:.4}",
            ea[n].len(),
            eb[n].len(),
            dot(&ea[n], &eb[n])
        );
    }
    for i in 0..ea.len() {
        for j in i + 1..ea.len() {
            println!(
                "clips {i},{j}: A {:.3}  B {:.3}",
                dot(&ea[i], &ea[j]),
                dot(&eb[i], &eb[j])
            );
        }
    }
    Ok(())
}
