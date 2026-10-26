//! logbook: the client of logbook-server. `play` streams a file at the pace
//! of speech, `live` the microphone and what the computer plays, `bench`
//! many files at once to find how many meetings a server keeps up with.
//! With `--final` the server reads the whole meeting again once it ends;
//! with `--brainy` the result becomes a page in brainy. A meeting with a
//! final pass is also kept here until it is published (pending.rs), and
//! `finish` publishes the ones a lost server left behind.

mod aec;
mod audio;
mod brainy;
mod client;
mod pending;
mod transcript;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset, Local};
use clap::{Parser, Subcommand};
use logbook_engine::protocol::{ChannelConfig, Utterance};

use crate::client::{Finalized, Pass, Server};
use crate::pending::{Meeting, Pending, Saved, Spool};

#[derive(Parser)]
#[command(version, about = "Live meeting transcription")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "LOGBOOK_SERVER",
        default_value = "http://127.0.0.1:3300"
    )]
    server: String,
    /// Where meetings wait until their transcript is published; by default
    /// `logbook` in the user's data folder.
    #[arg(long, global = true, env = "LOGBOOK_DATA")]
    data: Option<PathBuf>,
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
    /// Have the final transcript cleaned up by the server's language model,
    /// if it has one (fillers, false starts, punctuation). The model reads
    /// the meeting's text; without this it stays as heard.
    #[arg(long)]
    polish: bool,
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
    /// The page the meeting goes under: its address (copied from the
    /// browser), its id, or its path of titles, e.g. "Engineering/Meetings";
    /// by default the top of your default workspace.
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

    /// What is kept with a pending meeting: everything but the token.
    fn saved(&self) -> Saved {
        Saved {
            brainy: self.brainy,
            brainy_url: self.brainy_url.clone(),
            parent: self.parent.clone(),
            title: self.title.clone(),
            me: self.me.clone(),
        }
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
        /// hears. Without it, the call is cancelled from the microphone
        /// (aec.rs) and its words picked up anyway are dropped as echo.
        #[arg(long)]
        headphones: bool,
        /// Do not cancel the call from the microphone (to compare): only
        /// the server's word filter takes its echo out.
        #[arg(long)]
        no_aec: bool,
        /// Raise the microphone's level: "auto" follows the voice (a quiet
        /// room, someone far from the mic), a number is that many dB (0 to
        /// 50), "off" keeps the device's level.
        #[arg(long, default_value = "off", value_name = "auto|DB|off")]
        mic_gain: aec::Gain,
        #[command(flatten)]
        publish: Publish,
    },
    /// Publish the meetings left pending (the server could not be reached,
    /// or brainy refused the page): all of them, or the one in DIR.
    Finish {
        dir: Option<PathBuf>,
        /// brainy's address, if not the one given when the meeting started.
        #[arg(long, env = "BRAINY_URL")]
        brainy_url: Option<String>,
        #[arg(long, env = "BRAINY_TOKEN", hide_env_values = true)]
        brainy_token: Option<String>,
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
async fn main() -> ExitCode {
    // One line, causes included, instead of `Result`'s debug dump.
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let server = Server::new(&cli.server);
    let data = match cli.data {
        Some(dir) => dir,
        None => dirs::data_dir()
            .context("no data folder for this user: pass --data")?
            .join("logbook"),
    };
    match cli.command {
        Command::Play {
            file,
            speed,
            no_diarize,
            publish,
        } => play(&server, &data, &file, speed, !no_diarize, &publish).await,
        Command::Live {
            system,
            no_mic,
            diarize_mic,
            mic_device,
            system_device,
            headphones,
            no_aec,
            mic_gain,
            publish,
        } => {
            let mut sources = Vec::new();
            if !no_mic {
                sources.push((audio::Source::Microphone, mic_device, diarize_mic));
            }
            if system {
                sources.push((audio::Source::System, system_device, true));
            }
            let echo = match (system && !no_mic && !headphones, no_aec) {
                (false, _) => Echo::None,
                (true, true) => Echo::Filter,
                (true, false) => Echo::Cancel,
            };
            let gain = if no_mic { aec::Gain::Off } else { mic_gain };
            live(&server, &data, sources, echo, gain, &publish).await
        }
        Command::Finish {
            dir,
            brainy_url,
            brainy_token,
        } => {
            let dirs = match dir {
                Some(dir) => vec![dir],
                None => Pending::list(&data)?,
            };
            if dirs.is_empty() {
                eprintln!("no meeting is pending");
            }
            let brainy = Credentials {
                url: brainy_url,
                token: brainy_token,
            };
            let mut failed = 0;
            for dir in dirs {
                eprintln!("{}:", dir.display());
                let result = async {
                    let mut pending = Pending::open(&dir)?;
                    finish(&server, &mut pending, &brainy).await
                }
                .await;
                if let Err(error) = result {
                    eprintln!("  {error:#}");
                    failed += 1;
                }
            }
            if failed > 0 {
                bail!("{failed} meeting(s) still pending");
            }
            Ok(())
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
        } => bench(&server, &file, sessions, stagger, !no_diarize).await,
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

/// Prints the live transcript; with a final pass, hands its segments to
/// the publisher as they come.
struct Printer {
    start: Instant,
    speed: f64,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Progress>>,
}

impl client::Events for Printer {
    fn utterance(&mut self, u: Utterance) {
        println!(
            "[{}] {:<10} {}   (+{:.2} s)",
            clock(u.start),
            label(&u),
            u.text,
            latency(self.start, self.speed, &u)
        );
    }

    fn final_segments(&mut self, until: f64, utterances: Vec<Utterance>) {
        if let Some(tx) = &self.progress {
            let _ = tx.send(Progress::Segments {
                until: until.min(END),
                utterances,
            });
        }
    }

    fn finalizing(&mut self, audio_s: f64, id: &str) {
        eprintln!("finishing the final pass ({})…", clock(audio_s));
        if let Some(tx) = &self.progress {
            let _ = tx.send(Progress::FinalId(id.to_string()));
        }
    }
}

/// "Up to the end" for `published_until`: JSON has no infinity.
const END: f64 = f64::MAX;

enum Progress {
    Segments {
        until: f64,
        utterances: Vec<Utterance>,
    },
    FinalId(String),
}

/// The brainy address and token given now; the address falls back to the
/// one saved with the meeting, the token is never saved.
struct Credentials {
    url: Option<String>,
    token: Option<String>,
}

impl Credentials {
    /// None without --brainy; an error if it lacks its address or token.
    fn brainy(&self, saved: &Saved) -> Result<Option<brainy::Brainy>> {
        if !saved.brainy {
            return Ok(None);
        }
        let url = self
            .url
            .as_deref()
            .or(saved.brainy_url.as_deref())
            .context("--brainy needs BRAINY_URL")?;
        let token = self
            .token
            .as_deref()
            .context("--brainy needs BRAINY_TOKEN")?;
        Ok(Some(brainy::Brainy::new(url, token)?))
    }
}

fn started(pending: &Pending) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(&pending.meeting.started)
        .context("meeting.json: started is not a date")
}

/// Writes the segments that start between what the page already has and
/// `limit` to the meeting's brainy page, creating the page first. What
/// is written is recorded in the pending folder, so nothing is written
/// twice, whatever fails.
async fn write_page(
    b: &brainy::Brainy,
    pending: &mut Pending,
    limit: f64,
    segments: &[Utterance],
) -> Result<()> {
    let saved = pending.meeting.publish.clone();
    if pending.meeting.page.is_none() {
        let started = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
        let title = saved
            .title
            .clone()
            .unwrap_or_else(|| format!("Meeting {started}"));
        let page = b
            .create_page(
                &title,
                saved.parent.as_deref(),
                &format!(
                    "Recording started {started} · the transcript follows the meeting, \
                     a minute or so behind\n\n---\n"
                ),
            )
            .await?;
        eprintln!("brainy page: {}", page.url);
        pending.meeting.page = Some(page);
        pending.save()?;
    }
    let page = pending.meeting.page.clone().expect("created above");
    let new: Vec<Utterance> = segments
        .iter()
        .filter(|u| u.start >= pending.meeting.published_until && u.start < limit)
        .cloned()
        .collect();
    let mut writer = pending
        .meeting
        .writer
        .clone()
        .unwrap_or_else(|| transcript::Writer::new(&saved.me));
    if !new.is_empty() {
        b.append(&page.id, &writer.body(&new)).await?;
    }
    pending.meeting.writer = Some(writer);
    pending.meeting.published_until = pending.meeting.published_until.max(limit);
    pending.save()
}

/// The meeting's final segments as they come: kept in the pending folder,
/// and written to brainy with --brainy. A failed write is tried again with
/// the next segments. Gives the folder back when the meeting is over.
async fn publisher(
    mut pending: Pending,
    brainy: Option<brainy::Brainy>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Progress>,
) -> Pending {
    while let Some(progress) = rx.recv().await {
        let written = async {
            match progress {
                Progress::FinalId(id) => pending.set_final_id(Some(id)),
                Progress::Segments { until, utterances } => {
                    pending.add_segments(&utterances)?;
                    match &brainy {
                        Some(b) => {
                            let segments = pending.segments()?;
                            write_page(b, &mut pending, until, &segments).await
                        }
                        None => Ok(()),
                    }
                }
            }
        }
        .await;
        if let Err(error) = written {
            eprintln!("{error:#} (tried again with the next segments)");
        }
    }
    pending
}

/// The final transcript, complete: printed, or the rest written to the
/// brainy page and its header line rewritten.
async fn publish(pending: &mut Pending, brainy: &Credentials, finalized: &Finalized) -> Result<()> {
    let saved = pending.meeting.publish.clone();
    let started_text = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
    let duration_s = finalized
        .utterances
        .iter()
        .map(|u| u.end)
        .fold(0.0, f64::max);
    let Some(b) = brainy.brainy(&saved)? else {
        let markdown = transcript::markdown(&transcript::Meeting {
            utterances: &finalized.utterances,
            warnings: &finalized.warnings,
            me: &saved.me,
            started: &started_text,
            duration_s,
        });
        println!("\n{markdown}");
        return Ok(());
    };
    write_page(&b, pending, END, &finalized.utterances).await?;
    let writer = pending.meeting.writer.clone().unwrap_or_default();
    let mut header = writer
        .header(&started_text, duration_s, &[])
        .trim()
        .to_string();
    if !finalized.warnings.is_empty() {
        header.push_str(&format!(" · Note: {}", finalized.warnings.join("; ")));
    }
    let page = pending.meeting.page.clone().expect("written above");
    b.rewrite_first_block(&page.id, &header).await?;
    eprintln!("written to brainy: {}", page.url);
    Ok(())
}

/// Brings a pending meeting to its end: its final pass (complete over the
/// session, the one the server is running, or a new one over the audio
/// kept here), then the transcript published and the folder removed. On
/// any failure the folder stays, with whatever was reached, for
/// `logbook finish`.
async fn finish(server: &Server, pending: &mut Pending, brainy: &Credentials) -> Result<()> {
    let kept = |error: anyhow::Error, dir: &Path| {
        error.context(format!(
            "the meeting is kept in {}; `logbook finish` tries again",
            dir.display()
        ))
    };
    let finalized = match pending.final_result()? {
        Some(f) => f,
        None => match final_pass(server, pending).await {
            Ok(f) => {
                pending.save_final(&f)?;
                f
            }
            Err(error) => return Err(kept(error, &pending.dir)),
        },
    };
    match publish(pending, brainy, &finalized).await {
        Ok(()) => pending.remove(),
        Err(error) => {
            let started_text = started(pending)?.format("%Y-%m-%d %H:%M").to_string();
            let markdown = transcript::markdown(&transcript::Meeting {
                utterances: &finalized.utterances,
                warnings: &finalized.warnings,
                me: &pending.meeting.publish.me,
                started: &started_text,
                duration_s: finalized
                    .utterances
                    .iter()
                    .map(|u| u.end)
                    .fold(0.0, f64::max),
            });
            let file = pending.save_transcript(&markdown)?;
            Err(kept(
                error.context(format!(
                    "not written to brainy; the transcript is {}",
                    file.display()
                )),
                &pending.dir,
            ))
        }
    }
}

async fn final_pass(server: &Server, pending: &mut Pending) -> Result<Finalized> {
    if let Some(id) = pending.meeting.final_id.clone() {
        eprintln!("waiting for the final pass…");
        match client::poll(server, &id).await? {
            Pass::Done(f) => return Ok(f),
            Pass::Unknown => eprintln!("the server no longer has the final pass"),
            Pass::Failed(message) => eprintln!("{message}"),
        }
        pending.set_final_id(None)?;
    }
    let audio = pending.audio()?;
    let meeting = &pending.meeting;
    let id = client::upload(server, meeting.channels.clone(), meeting.polish, audio).await?;
    pending.set_final_id(Some(id.clone()))?;
    eprintln!("reading the whole meeting again…");
    match client::poll(server, &id).await? {
        Pass::Done(f) => Ok(f),
        Pass::Unknown => bail!("the server lost the final pass it had just started"),
        Pass::Failed(message) => {
            pending.set_final_id(None)?;
            bail!("{message}")
        }
    }
}

/// A meeting being recorded: the audio of `audio` goes to the server, and,
/// with a final pass, to a pending folder too, where the final segments
/// are kept as they come (and written to brainy with --brainy). Returns
/// when the audio ends and the meeting is finished.
async fn meeting(
    server: &Server,
    data: &Path,
    channels: Vec<ChannelConfig>,
    p: &Publish,
    mut audio: client::Audio,
    start: Instant,
    speed: f64,
) -> Result<()> {
    let started = Local::now();
    let credentials = Credentials {
        url: p.brainy_url.clone(),
        token: p.brainy_token.clone(),
    };
    let mut spool: Option<Spool> = None;
    let mut publishing = None;
    let mut printer = Printer {
        start,
        speed,
        progress: None,
    };
    if p.final_pass() {
        // A missing token or a parent that names no page fails now, not
        // at the end of the meeting. The parent is kept as its id: a page
        // renamed before `finish` still takes the meeting.
        let mut publish = p.saved();
        let brainy = credentials.brainy(&publish)?;
        if let (Some(b), Some(parent)) = (&brainy, &publish.parent) {
            publish.parent = Some(b.resolve_page(parent).await.context("--parent")?);
        }
        let meeting = Meeting {
            started: started.to_rfc3339(),
            channels: channels.clone(),
            polish: p.polish,
            publish,
            final_id: None,
            page: None,
            writer: None,
            published_until: 0.0,
        };
        let (created, writer) =
            Pending::create(data, &started.format("%Y%m%d-%H%M%S").to_string(), meeting)?;
        eprintln!("keeping the meeting in {}", created.dir.display());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        printer.progress = Some(tx);
        publishing = Some(tokio::spawn(publisher(created, brainy, rx)));
        spool = Some(writer);
    }
    // Every frame is written here before it goes to the server.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tee = tokio::spawn(async move {
        let mut failed = None;
        while let Some((channel, samples)) = audio.recv().await {
            if let Some(s) = &mut spool
                && let Err(error) = s.write(channel as usize, &samples)
            {
                eprintln!("could not keep the meeting's audio: {error:#}");
                failed = Some(error);
                spool = None;
            }
            if tx.send((channel, samples)).is_err() {
                break;
            }
        }
        match (failed, spool) {
            (Some(error), _) => Err(error),
            (None, Some(s)) => s.finish(),
            (None, None) => Ok(()),
        }
    });
    let final_pass = p.final_pass().then_some(p.polish);
    let live = client::live(server, channels, final_pass, rx, &mut printer).await;
    drop(printer);
    let kept = tee.await?;
    let Some(publishing) = publishing else {
        if !live.intact {
            bail!("the connection was lost: the live transcript has gaps");
        }
        return Ok(());
    };
    let mut pending = publishing.await?;
    if let Some(warnings) = live.finalized {
        let utterances = pending.segments()?;
        pending.save_final(&Finalized {
            utterances,
            warnings,
        })?;
    } else if let Err(error) = kept {
        eprintln!("the final pass reads only what was kept before: {error:#}");
    }
    finish(server, &mut pending, &credentials).await
}

async fn play(
    server: &Server,
    data: &Path,
    file: &Path,
    speed: f64,
    diarize: bool,
    p: &Publish,
) -> Result<()> {
    let samples = Arc::new(audio::decode(file)?);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let channels = vec![ChannelConfig {
        name: "system".into(),
        diarize,
        echo_of: None,
    }];
    let start = Instant::now();
    tokio::spawn(audio::play(samples, speed, start, tx));
    meeting(server, data, channels, p, rx, start, speed).await
}

/// What the microphone hears of the system channel.
#[derive(Clone, Copy, PartialEq)]
enum Echo {
    /// Nothing: headphones, or one of the two channels only.
    None,
    /// The call: the server drops its words from the mic's.
    Filter,
    /// The call: cancelled from the mic's audio first, then filtered.
    Cancel,
}

/// Each source is a channel: (what, which device, diarized).
async fn live(
    server: &Server,
    data: &Path,
    sources: Vec<(audio::Source, Option<String>, bool)>,
    echo: Echo,
    gain: aec::Gain,
    p: &Publish,
) -> Result<()> {
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
        let echo_of = match source {
            audio::Source::Microphone if echo != Echo::None => Some("system".into()),
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
    // The microphone is channel 0 and the system channel 1 when there are both.
    if echo == Echo::Cancel {
        eprintln!("cancelling the call's echo from the microphone");
    }
    if gain != aec::Gain::Off {
        eprintln!("raising the microphone's level: {gain:?}");
    }
    let rx = match (echo == Echo::Cancel, gain) {
        (false, aec::Gain::Off) => rx,
        (cancel, gain) => aec::process(rx, 0, cancel.then_some(1), gain),
    };
    eprintln!("recording; Ctrl-C to stop");
    let stop = Mutex::new(stops);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("stopping…");
        stop.lock().unwrap().clear();
    });
    meeting(server, data, channels, p, rx, start, 1.0).await
}

async fn bench(
    server: &Server,
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
        let server = server.clone();
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
            let mut events = Latencies { start, results };
            let live = client::live(&server, channels, None, rx, &mut events).await;
            if !live.intact {
                bail!("the connection was lost");
            }
            anyhow::Ok(())
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

/// (audio time of the phrase's end, latency) of every utterance.
struct Latencies {
    start: Instant,
    results: Arc<Mutex<Vec<(f64, f64)>>>,
}

impl client::Events for Latencies {
    fn utterance(&mut self, u: Utterance) {
        self.results
            .lock()
            .unwrap()
            .push((u.end, latency(self.start, 1.0, &u)));
    }
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
