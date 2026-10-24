//! logbook: the client of logbook-server. `play` streams a file at the pace
//! of speech, `live` the microphone and what the computer plays, `bench`
//! many files at once to find how many meetings a server keeps up with.
//! With `--final` the server reads the whole meeting again once it ends;
//! with `--brainy` the result becomes a page in brainy.

mod audio;
mod brainy;
mod client;
mod transcript;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
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

/// What happens once the meeting ends.
#[derive(clap::Args)]
struct Publish {
    /// Read the whole meeting again once it ends (better speakers and
    /// phrases) and print the final transcript as Markdown.
    #[arg(long = "final")]
    final_pass: bool,
    /// Write the final transcript as a page in brainy (implies --final).
    #[arg(long)]
    brainy: bool,
    /// brainy's address, e.g. https://brainy.example.com.
    #[arg(long, env = "BRAINY_URL", requires = "brainy")]
    brainy_url: Option<String>,
    /// A personal token of yours (brainy: Settings → Connections), with write access.
    #[arg(
        long,
        env = "BRAINY_TOKEN",
        hide_env_values = true,
        requires = "brainy"
    )]
    brainy_token: Option<String>,
    /// The page the meeting goes under (its id); by default the top of your
    /// default workspace.
    #[arg(long, env = "LOGBOOK_PARENT", requires = "brainy")]
    parent: Option<String>,
    /// The page's title; by default "Meeting" and the date.
    #[arg(long)]
    title: Option<String>,
    /// Your name, for what the microphone hears.
    #[arg(long, env = "LOGBOOK_ME", default_value = "Me")]
    me: String,
}

impl Publish {
    fn final_pass(&self) -> bool {
        self.final_pass || self.brainy
    }
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
        #[command(flatten)]
        publish: Publish,
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
        /// The microphone does not hear the speakers: keep every word it
        /// hears. Without it, the call's words picked up by the microphone
        /// are dropped as echo.
        #[arg(long)]
        headphones: bool,
        #[command(flatten)]
        publish: Publish,
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
            publish,
        } => play(&cli.server, &file, speed, !no_diarize, &publish).await,
        Command::Live {
            system,
            no_mic,
            diarize_mic,
            mic_device,
            system_device,
            headphones,
            publish,
        } => {
            let mut sources = Vec::new();
            if !no_mic {
                sources.push((audio::Source::Microphone, mic_device, diarize_mic));
            }
            if system {
                sources.push((audio::Source::System, system_device, true));
            }
            live(&cli.server, sources, system && !headphones, &publish).await
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

fn print_live(start: Instant, speed: f64) -> impl FnMut(Utterance) {
    move |u| {
        println!(
            "[{}] {:<10} {}   (+{:.2} s)",
            clock(u.start),
            label(&u),
            u.text,
            latency(start, speed, &u)
        );
    }
}

fn print_finalizing(audio_s: f64) {
    eprintln!("reading the whole meeting again ({})…", clock(audio_s));
}

/// The final transcript: printed, or written to brainy. If brainy refuses
/// it, it is saved next to you rather than lost.
async fn publish(
    p: &Publish,
    started: DateTime<Local>,
    finalized: client::Finalized,
) -> Result<()> {
    let started_text = started.format("%Y-%m-%d %H:%M").to_string();
    let duration_s = finalized
        .utterances
        .iter()
        .map(|u| u.end)
        .fold(0.0, f64::max);
    let markdown = transcript::markdown(&transcript::Meeting {
        utterances: &finalized.utterances,
        warnings: &finalized.warnings,
        me: &p.me,
        started: &started_text,
        duration_s,
    });
    if !p.brainy {
        println!("\n{markdown}");
        return Ok(());
    }
    let title = p
        .title
        .clone()
        .unwrap_or_else(|| format!("Meeting {started_text}"));
    let written = async {
        let url = p
            .brainy_url
            .as_deref()
            .context("--brainy needs BRAINY_URL")?;
        let token = p
            .brainy_token
            .as_deref()
            .context("--brainy needs BRAINY_TOKEN")?;
        brainy::Brainy::new(url, token)?
            .create_page(&title, p.parent.as_deref(), &markdown)
            .await
    }
    .await;
    match written {
        Ok(url) => {
            eprintln!("written to brainy: {url}");
            Ok(())
        }
        Err(error) => {
            let file = format!("logbook-{}.md", started.format("%Y%m%d-%H%M%S"));
            std::fs::write(&file, format!("# {title}\n\n{markdown}"))
                .with_context(|| format!("could not save {file} either"))?;
            Err(error.context(format!("not written to brainy; saved as {file}")))
        }
    }
}

async fn play(server: &str, file: &Path, speed: f64, diarize: bool, p: &Publish) -> Result<()> {
    let samples = Arc::new(audio::decode(file)?);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let channels = vec![ChannelConfig {
        name: "system".into(),
        diarize,
        echo_of: None,
    }];
    let start = Instant::now();
    let started = Local::now();
    tokio::spawn(audio::play(samples, speed, start, tx));
    let finalized = client::run(
        server,
        channels,
        p.final_pass(),
        rx,
        print_live(start, speed),
        print_finalizing,
    )
    .await?;
    match finalized {
        Some(f) => publish(p, started, f).await,
        None => Ok(()),
    }
}

/// Each source is a channel: (what, which device, diarized).
/// `echo`: the microphone may hear the system channel again.
async fn live(
    server: &str,
    sources: Vec<(audio::Source, Option<String>, bool)>,
    echo: bool,
    p: &Publish,
) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stops = Vec::new();
    let mut channels = Vec::new();
    // Every channel's sample 0, and what latencies are measured from.
    let start = Instant::now();
    let started = Local::now();
    for (index, (source, device, diarize)) in sources.into_iter().enumerate() {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let name = audio::capture(source, device, index as u8, start, tx.clone(), stop_rx)?;
        let channel = match source {
            audio::Source::Microphone => "mic",
            audio::Source::System => "system",
        };
        eprintln!("{channel}: {name}");
        stops.push(stop_tx);
        let echo_of = match source {
            audio::Source::Microphone if echo => Some("system".into()),
            _ => None,
        };
        channels.push(ChannelConfig {
            name: channel.into(),
            diarize,
            echo_of,
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
    let finalized = client::run(
        server,
        channels,
        p.final_pass(),
        rx,
        print_live(start, 1.0),
        print_finalizing,
    )
    .await?;
    match finalized {
        Some(f) => publish(p, started, f).await,
        None => Ok(()),
    }
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
                echo_of: None,
            }];
            let start = Instant::now();
            tokio::spawn(audio::play(samples, 1.0, start, tx));
            client::run(
                &server,
                channels,
                false,
                rx,
                |u| {
                    results
                        .lock()
                        .unwrap()
                        .push((u.end, latency(start, 1.0, &u)));
                },
                |_| {},
            )
            .await
            .map(|_| ())
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
