//! logbook: the client of logbook-server. `play` streams a file at the pace
//! of speech, `live` the microphone and what the computer plays, `bench`
//! many files at once to find how many meetings a server keeps up with.

mod audio;
mod client;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use logbook_engine::protocol::{ChannelConfig, Utterance};

#[derive(Parser)]
#[command(version, about = "Live meeting transcription")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "LOGBOOK_SERVER",
        default_value = "ws://127.0.0.1:3300/live"
    )]
    server: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Stream a recording as if it were being spoken, and print the transcript.
    Play {
        file: PathBuf,
        /// 2 plays twice as fast.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Do not tell the speakers apart.
        #[arg(long)]
        no_diarize: bool,
    },
    /// Transcribe the microphone, and with --system the call, until Ctrl-C.
    Live {
        /// Also record what the computer plays (the call's other people), as
        /// a second channel whose speakers are told apart.
        #[arg(long)]
        system: bool,
        /// Record only what the computer plays.
        #[arg(long, requires = "system")]
        no_mic: bool,
        /// Tell the speakers apart on the microphone too (a room; not your
        /// own voice on headphones).
        #[arg(long)]
        diarize_mic: bool,
        /// Microphone by part of its name (`logbook devices`).
        #[arg(long)]
        mic_device: Option<String>,
        /// Output device (or Linux monitor) by part of its name.
        #[arg(long)]
        system_device: Option<String>,
    },
    /// List the audio devices live can record from.
    Devices,
    /// Stream one recording as many sessions at once; report the latency.
    Bench {
        file: PathBuf,
        #[arg(long, default_value_t = 4)]
        sessions: usize,
        /// Seconds between the start of one session and the next, so that
        /// their phrases do not end in lockstep.
        #[arg(long, default_value_t = 0.37)]
        stagger: f64,
        #[arg(long)]
        no_diarize: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Play {
            file,
            speed,
            no_diarize,
        } => play(&cli.server, &file, speed, !no_diarize).await,
        Command::Live {
            system,
            no_mic,
            diarize_mic,
            mic_device,
            system_device,
        } => {
            let mut sources = Vec::new();
            if !no_mic {
                sources.push((audio::Source::Microphone, mic_device, diarize_mic));
            }
            if system {
                sources.push((audio::Source::System, system_device, true));
            }
            live(&cli.server, sources).await
        }
        Command::Devices => {
            for line in audio::devices()? {
                println!("{line}");
            }
            Ok(())
        }
        Command::Bench {
            file,
            sessions,
            stagger,
            no_diarize,
        } => bench(&cli.server, &file, sessions, stagger, !no_diarize).await,
    }
}

fn label(u: &Utterance) -> String {
    match u.speaker {
        Some(s) => format!("{} S{}", u.channel, s + 1),
        None => u.channel.clone(),
    }
}

fn clock(t: f64) -> String {
    format!("{:02}:{:04.1}", (t / 60.0) as u64, t % 60.0)
}

/// Seconds between the moment a phrase's last word was spoken and the moment
/// its text arrived.
fn latency(start: Instant, speed: f64, u: &Utterance) -> f64 {
    start.elapsed().as_secs_f64() - u.end / speed
}

async fn play(server: &str, file: &Path, speed: f64, diarize: bool) -> Result<()> {
    let samples = Arc::new(audio::decode(file)?);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let channels = vec![ChannelConfig {
        name: "system".into(),
        diarize,
    }];
    let start = Instant::now();
    tokio::spawn(audio::play(samples, speed, start, tx));
    client::run(server, channels, rx, |u| {
        println!(
            "[{}] {:<10} {}   (+{:.2} s)",
            clock(u.start),
            label(&u),
            u.text,
            latency(start, speed, &u)
        );
    })
    .await
}

/// Each source is a channel: (what, which device, diarized).
async fn live(server: &str, sources: Vec<(audio::Source, Option<String>, bool)>) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stops = Vec::new();
    let mut channels = Vec::new();
    // Every channel's sample 0, and what latencies are measured from.
    let start = Instant::now();
    for (index, (source, device, diarize)) in sources.into_iter().enumerate() {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let name = audio::capture(source, device, index as u8, start, tx.clone(), stop_rx)?;
        let channel = match source {
            audio::Source::Microphone => "mic",
            audio::Source::System => "system",
        };
        eprintln!("{channel}: {name}");
        stops.push(stop_tx);
        channels.push(ChannelConfig {
            name: channel.into(),
            diarize,
        });
    }
    // The session stops when the last sender is gone: the capture threads'.
    drop(tx);
    eprintln!("recording; Ctrl-C to stop");
    let stop = Mutex::new(stops);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("stopping…");
        stop.lock().unwrap().clear();
    });
    client::run(server, channels, rx, |u| {
        println!(
            "[{}] {:<10} {}   (+{:.2} s)",
            clock(u.start),
            label(&u),
            u.text,
            latency(start, 1.0, &u)
        );
    })
    .await
}

async fn bench(
    server: &str,
    file: &Path,
    sessions: usize,
    stagger: f64,
    diarize: bool,
) -> Result<()> {
    let samples = Arc::new(audio::decode(file)?);
    let duration = samples.len() as f64 / audio::SAMPLE_RATE as f64;
    eprintln!(
        "{sessions} sessions of {duration:.1} s, diarization {}",
        if diarize { "on" } else { "off" }
    );
    // (audio time of the phrase's end, latency)
    let results: Arc<Mutex<Vec<(f64, f64)>>> = Arc::default();
    let mut tasks = Vec::new();
    for i in 0..sessions {
        let samples = samples.clone();
        let results = results.clone();
        let server = server.to_string();
        let delay = std::time::Duration::from_secs_f64(stagger * i as f64);
        tasks.push(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let channels = vec![ChannelConfig {
                name: "system".into(),
                diarize,
            }];
            let start = Instant::now();
            tokio::spawn(audio::play(samples, 1.0, start, tx));
            client::run(&server, channels, rx, |u| {
                results
                    .lock()
                    .unwrap()
                    .push((u.end, latency(start, 1.0, &u)));
            })
            .await
        }));
    }
    let mut failed = 0;
    for task in tasks {
        if let Err(error) = task.await? {
            eprintln!("a session failed: {error:#}");
            failed += 1;
        }
    }
    let results = results.lock().unwrap();
    let mut all: Vec<f64> = results.iter().map(|r| r.1).collect();
    println!("sessions        {sessions} ({failed} failed), {duration:.1} s each");
    println!("utterances      {}", all.len());
    if all.is_empty() {
        return Ok(());
    }
    println!("latency         {}", summary(&mut all));
    // If the server does not keep up, the latency grows along the recording.
    for q in 0..4 {
        let (from, to) = (duration * q as f64 / 4.0, duration * (q + 1) as f64 / 4.0);
        let mut part: Vec<f64> = results
            .iter()
            .filter(|r| r.0 >= from && r.0 < to)
            .map(|r| r.1)
            .collect();
        if !part.is_empty() {
            println!("  {}–{}  {}", clock(from), clock(to), summary(&mut part));
        }
    }
    Ok(())
}

fn summary(values: &mut [f64]) -> String {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    format!(
        "p50 {:.2} s   p90 {:.2} s   p99 {:.2} s   max {:.2} s",
        at(0.5),
        at(0.9),
        at(0.99),
        at(1.0)
    )
}
