//! Diarizes a recording with one of several strategies and prints RTTM, to
//! be scored against a reference (scripts/der.py). How the final pass's
//! speakers were chosen: docs/decisions/2026-10-24-speakers-across-a-long-meeting.md.
//!
//! cargo run --release -p logbook-engine --example diarize -- \
//!     --lib vendor/libparakeet.dylib --diar MODEL.gguf [--voice BUNDLE.gguf] \
//!     --strategy offline|stream-low|stream-model|chunked|stream-embed|stream-model-embed \
//!     [--window 120] [--threshold 0.5] [--recluster 0.5] FILE > out.rttm
//!
//! Timing goes to stderr.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use logbook_engine::parakeet::{Kind, Latency, Model, Parakeet, SAMPLE_RATE, Turn};

struct Args {
    lib: PathBuf,
    diar: PathBuf,
    voice: Option<PathBuf>,
    strategy: String,
    window: f64,
    threshold: f32,
    recluster: Option<f32>,
    /// With a stream's slots: keep a slot's meeting speaker while the voice
    /// is at least this similar to it.
    prior: Option<f32>,
    /// Seconds of audio per voice embedding.
    embed: f64,
    share: Option<f32>,
    file: PathBuf,
}

fn args() -> Result<Args> {
    let mut a = std::env::args().skip(1);
    let mut lib = None;
    let mut diar = None;
    let mut voice = None;
    let mut strategy = "offline".to_string();
    let mut window = 120.0;
    let mut threshold = 0.5;
    let mut recluster = None;
    let mut prior = None;
    let mut embed = 10.0;
    let mut share = None;
    let mut file = None;
    while let Some(arg) = a.next() {
        let mut value = || a.next().context("a value is missing");
        match arg.as_str() {
            "--lib" => lib = Some(value()?.into()),
            "--diar" => diar = Some(value()?.into()),
            "--voice" => voice = Some(value()?.into()),
            "--strategy" => strategy = value()?,
            "--window" => window = value()?.parse()?,
            "--threshold" => threshold = value()?.parse()?,
            "--recluster" => recluster = Some(value()?.parse()?),
            "--prior" => prior = Some(value()?.parse()?),
            "--embed" => embed = value()?.parse()?,
            "--share" => share = Some(value()?.parse()?),
            _ => file = Some(arg.into()),
        }
    }
    Ok(Args {
        lib: lib.context("--lib")?,
        diar: diar.context("--diar")?,
        voice,
        strategy,
        window,
        threshold,
        recluster,
        prior,
        embed,
        share,
        file: file.context("FILE")?,
    })
}

fn decode(path: &Path) -> Result<Vec<f32>> {
    let out = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(path)
        .args(["-ar", "16000", "-ac", "1", "-f", "s16le", "-"])
        .output()
        .context("ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(out
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}

fn main() -> Result<()> {
    let args = args()?;
    let lib = Parakeet::open(Some(&args.lib))?;
    let diar = Model::load(&lib, &args.diar, Kind::Diarization)?;
    let voice = match &args.voice {
        Some(path) => Some(
            Model::load(&lib, path, Kind::Speaker)
                .or_else(|_| Model::load_component(&lib, path, Some("voice"), Kind::Speaker))?,
        ),
        None => None,
    };
    let pcm = decode(&args.file)?;
    let duration = pcm.len() as f64 / SAMPLE_RATE as f64;
    let started = Instant::now();
    let turns = match args.strategy.as_str() {
        "offline" => diar.diarize(&pcm)?,
        "stream-low" => stream(&diar, &pcm, Latency::Low)?,
        "stream-model" => stream(&diar, &pcm, Latency::Model)?,
        // The final pass's own code: each window's speakers (the stream's
        // slots, or a diarization of the window alone) matched by voice
        // (logbook_engine::speakers).
        "linker" | "chunked-linker" => {
            let voice = voice.as_ref().context("--voice")?;
            let windows = if args.strategy == "linker" {
                split(&stream(&diar, &pcm, Latency::Model)?, duration, args.window)
            } else {
                chunked(&diar, &pcm, args.window)?
            };
            let mut linker = logbook_engine::speakers::Linker::new(args.threshold);
            if let Some(share) = args.share {
                linker = linker.with_share(share);
            }
            let mut out = Vec::new();
            let embedding = Instant::now();
            for (from, to, local) in windows {
                let (i, j) = (
                    (from * SAMPLE_RATE as f64) as usize,
                    ((to * SAMPLE_RATE as f64) as usize).min(pcm.len()),
                );
                let voices = logbook_engine::speakers::voices_up_to(
                    voice,
                    &pcm[i..j],
                    from,
                    &local,
                    args.embed,
                )?;
                let map = linker.link(&voices);
                out.extend(
                    local
                        .iter()
                        .filter_map(|t| map.get(&t.speaker).map(|&speaker| Turn { speaker, ..*t })),
                );
            }
            eprintln!(
                "speakers {}; voices took {:.1} s",
                linker.speakers(),
                embedding.elapsed().as_secs_f64()
            );
            out
        }
        "chunked" | "stream-embed" | "stream-model-embed" => {
            let voice = voice.as_ref().context("--voice")?;
            let mut linker = Linker::new(args.threshold);
            // Slots carry over only between a stream's windows.
            linker.prior = args.prior.filter(|_| args.strategy != "chunked");
            let windows = match args.strategy.as_str() {
                "chunked" => chunked(&diar, &pcm, args.window)?,
                "stream-embed" => split(&stream(&diar, &pcm, Latency::Low)?, duration, args.window),
                _ => split(&stream(&diar, &pcm, Latency::Model)?, duration, args.window),
            };
            let mut out = Vec::new();
            for (from, to, local) in windows {
                out.extend(linker.link(voice, &pcm, from, to, &local)?);
            }
            if let Some(threshold) = args.recluster {
                linker.recluster(&mut out, threshold);
            }
            eprintln!(
                "speakers {} (embedded {} window-speakers)",
                linker.centroids.len(),
                linker.embeddings.len()
            );
            out
        }
        other => bail!("unknown strategy {other}"),
    };
    let took = started.elapsed().as_secs_f64();
    eprintln!(
        "{}: {duration:.0} s in {took:.1} s ({:.0}x real time), {} turns",
        args.strategy,
        duration / took,
        turns.len()
    );
    let name = args
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for t in turns {
        println!(
            "SPEAKER {name} 1 {:.3} {:.3} <NA> <NA> spk{} <NA> <NA>",
            t.start,
            t.end - t.start,
            t.speaker
        );
    }
    Ok(())
}

/// The whole recording through a stream, a second at a time.
fn stream(diar: &Arc<Model>, pcm: &[f32], latency: Latency) -> Result<Vec<Turn>> {
    let mut s = diar.diar_stream(latency)?;
    let mut turns = Vec::new();
    let step = SAMPLE_RATE as usize;
    let chunks: Vec<&[f32]> = pcm.chunks(step).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        turns.extend(s.feed(chunk, i + 1 == chunks.len())?);
    }
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(turns)
}

/// Windows of `window` seconds, each diarized alone: its speakers are its own.
fn chunked(diar: &Model, pcm: &[f32], window: f64) -> Result<Vec<(f64, f64, Vec<Turn>)>> {
    let n = (window * SAMPLE_RATE as f64) as usize;
    let mut out = Vec::new();
    for (i, chunk) in pcm.chunks(n).enumerate() {
        let from = i as f64 * window;
        let turns = diar
            .diarize(chunk)?
            .into_iter()
            .map(|t| Turn {
                start: t.start + from,
                end: t.end + from,
                ..t
            })
            .collect();
        out.push((from, from + chunk.len() as f64 / SAMPLE_RATE as f64, turns));
    }
    Ok(out)
}

/// A stream's turns cut into windows, each slot of each window a local speaker.
fn split(turns: &[Turn], duration: f64, window: f64) -> Vec<(f64, f64, Vec<Turn>)> {
    let mut out = Vec::new();
    let mut from = 0.0;
    while from < duration {
        let to = (from + window).min(duration);
        let local = turns
            .iter()
            .filter(|t| t.end > from && t.start < to)
            .map(|t| Turn {
                start: t.start.max(from),
                end: t.end.min(to),
                ..*t
            })
            .collect();
        out.push((from, to, local));
        from = to;
    }
    out
}

/// Speakers of a window matched to the meeting's by voice, as the meeting
/// goes: a local speaker joins the most similar known one above the
/// threshold, else is a new one.
struct Linker {
    threshold: f32,
    /// Sum of the embeddings matched to each meeting speaker (its direction
    /// is the centroid).
    centroids: Vec<Vec<f32>>,
    /// (window-speaker embedding, its weight in seconds, the meeting speaker
    /// it went to) for the final re-clustering.
    embeddings: Vec<(Vec<f32>, f64, u32)>,
    prior: Option<f32>,
    /// A stream slot's meeting speaker in the last window it spoke.
    slots: std::collections::HashMap<u32, usize>,
}

/// The least audio of a local speaker worth an embedding; shorter ones
/// take the nearest known speaker, whatever the similarity.
const MIN_EMBED: f64 = 1.5;
/// The most audio embedded per local speaker.
const MAX_EMBED: f64 = 30.0;

impl Linker {
    fn new(threshold: f32) -> Self {
        Self {
            threshold,
            centroids: Vec::new(),
            embeddings: Vec::new(),
            prior: None,
            slots: std::collections::HashMap::new(),
        }
    }

    fn link(
        &mut self,
        voice: &Model,
        pcm: &[f32],
        from: f64,
        to: f64,
        local: &[Turn],
    ) -> Result<Vec<Turn>> {
        let _ = (from, to);
        let mut speakers: Vec<u32> = local.iter().map(|t| t.speaker).collect();
        speakers.sort();
        speakers.dedup();
        // Each local speaker's audio where nobody else speaks.
        let mut candidates = Vec::new();
        for &s in &speakers {
            let mut audio = Vec::new();
            let mut seconds = 0.0;
            for t in local.iter().filter(|t| t.speaker == s) {
                for (a, b) in alone(t, local) {
                    if seconds >= MAX_EMBED {
                        break;
                    }
                    let b = b.min(a + MAX_EMBED - seconds);
                    let (i, j) = (
                        (a * SAMPLE_RATE as f64) as usize,
                        ((b * SAMPLE_RATE as f64) as usize).min(pcm.len()),
                    );
                    if j > i {
                        audio.extend_from_slice(&pcm[i..j]);
                        seconds += b - a;
                    }
                }
            }
            let embedding = if seconds >= MIN_EMBED {
                Some(voice.embed(&audio)?)
            } else {
                None
            };
            candidates.push((s, embedding, seconds));
        }
        // Greedy by similarity, longest speakers first: two local speakers
        // of one window never become the same meeting speaker.
        let mut taken = vec![false; self.centroids.len()];
        let mut map = std::collections::HashMap::new();
        candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
        for (s, embedding, seconds) in candidates {
            let best = |e: &[f32], taken: &[bool], centroids: &[Vec<f32>]| {
                centroids
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !taken[*i])
                    .map(|(i, c)| (i, cosine(e, c)))
                    .max_by(|a, b| a.1.total_cmp(&b.1))
            };
            // The slot's speaker of the last window, while the voice fits.
            let kept = self.prior.and_then(|floor| {
                let &g = self.slots.get(&s)?;
                if taken[g] {
                    return None;
                }
                match &embedding {
                    Some(e) if cosine(e, &self.centroids[g]) >= floor => Some(g),
                    None => Some(g),
                    _ => None,
                }
            });
            let global = match (&embedding, kept) {
                (Some(e), Some(g)) => {
                    add(&mut self.centroids[g], e, seconds);
                    g
                }
                (None, Some(g)) => g,
                (Some(e), None) => match best(e, &taken, &self.centroids) {
                    Some((i, sim)) if sim >= self.threshold => {
                        add(&mut self.centroids[i], e, seconds);
                        i
                    }
                    _ => {
                        self.centroids
                            .push(e.iter().map(|x| x * seconds as f32).collect());
                        taken.push(false);
                        self.centroids.len() - 1
                    }
                },
                // Too little audio to tell: the speaker's own slot index
                // is all there is; give it a new speaker only if none exist.
                (None, None) => {
                    if self.centroids.is_empty() {
                        continue;
                    }
                    match taken.iter().position(|t| !t) {
                        Some(i) => i,
                        None => continue,
                    }
                }
            };
            taken[global] = true;
            self.slots.insert(s, global);
            if let Some(e) = embedding {
                self.embeddings.push((e, seconds, global as u32));
            }
            map.insert(s, global as u32);
        }
        Ok(local
            .iter()
            .filter_map(|t| map.get(&t.speaker).map(|&g| Turn { speaker: g, ..*t }))
            .collect())
    }

    /// Agglomerative clustering of every window-speaker embedding (average
    /// linkage, stop below `threshold`), then every turn relabelled: what a
    /// final step can do once the meeting is over, on vectors only.
    fn recluster(&mut self, turns: &mut [Turn], threshold: f32) {
        let n = self.centroids.len();
        // Start from the online speakers: their summed embeddings.
        let mut clusters: Vec<(Vec<f32>, Vec<usize>)> = self
            .centroids
            .iter()
            .enumerate()
            .map(|(i, c)| (c.clone(), vec![i]))
            .collect();
        loop {
            let mut best = None;
            for i in 0..clusters.len() {
                for j in i + 1..clusters.len() {
                    let sim = cosine(&clusters[i].0, &clusters[j].0);
                    if best.is_none_or(|(_, _, b)| sim > b) {
                        best = Some((i, j, sim));
                    }
                }
            }
            match best {
                Some((i, j, sim)) if sim >= threshold => {
                    let (c, members) = clusters.remove(j);
                    for (a, b) in clusters[i].0.iter_mut().zip(&c) {
                        *a += b;
                    }
                    clusters[i].1.extend(members);
                }
                _ => break,
            }
        }
        let mut relabel = vec![0u32; n];
        for (k, (_, members)) in clusters.iter().enumerate() {
            for &m in members {
                relabel[m] = k as u32;
            }
        }
        for t in turns {
            t.speaker = relabel[t.speaker as usize];
        }
        eprintln!("reclustered {n} -> {}", clusters.len());
    }
}

/// The parts of `t` where no other local speaker talks.
fn alone(t: &Turn, all: &[Turn]) -> Vec<(f64, f64)> {
    let mut parts = vec![(t.start, t.end)];
    for o in all.iter().filter(|o| o.speaker != t.speaker) {
        parts = parts
            .into_iter()
            .flat_map(|(a, b)| {
                if o.end <= a || o.start >= b {
                    vec![(a, b)]
                } else {
                    let mut v = Vec::new();
                    if o.start > a {
                        v.push((a, o.start));
                    }
                    if o.end < b {
                        v.push((o.end, b));
                    }
                    v
                }
            })
            .collect();
    }
    parts
}

fn add(sum: &mut [f32], e: &[f32], weight: f64) {
    for (a, b) in sum.iter_mut().zip(e) {
        *a += b * weight as f32;
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}
