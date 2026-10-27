//! logbook: meetings transcribed live, in this process or, with
//! `--server`, by logbook-server (crates/client runs them; this file is
//! the command line). `play` streams a file at the pace of
//! speech, `live` the microphone and what the computer plays, `bench` many
//! files at once to find how many meetings a machine keeps up with.
//! With `--final` the meeting is read again as it goes and once it ends;
//! with `--brainy` the result becomes a page in brainy, with `--report`
//! a language model writes its report. A meeting with a final pass is
//! also kept here until it is published (client pending.rs), and
//! `finish` publishes the ones a lost server left behind.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use logbook_client::meeting::{self, Backend, Credentials, Outcome};
use logbook_client::pending::Pending;
use logbook_client::remote::{self, Server};
use logbook_client::{aec, audio, local, voices};
use logbook_engine::protocol::{ChannelConfig, Utterance};

#[derive(Parser)]
#[command(version, about = "Live meeting transcription")]
struct Cli {
    /// logbook-server, e.g. http://127.0.0.1:3300; without it the meeting
    /// is transcribed here, with the models fetched on first use.
    #[arg(long, global = true, env = "LOGBOOK_SERVER")]
    server: Option<String>,
    /// Your token on that server, made on its admin page (its address in a browser).
    #[arg(
        long,
        global = true,
        env = "LOGBOOK_SERVER_TOKEN",
        hide_env_values = true
    )]
    server_token: Option<String>,
    #[command(flatten)]
    local: LocalArgs,
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
    /// Have the language model (the server's, or LOGBOOK_POLISH_URL without
    /// one) write the meeting's report: summary, decisions, action items
    /// (implies --final). It reads the meeting's text.
    #[arg(long)]
    report: bool,
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
    fn publish(&self) -> meeting::Publish {
        meeting::Publish {
            final_pass: self.final_pass,
            polish: self.polish,
            report: self.report,
            keep: false,
            brainy: self.brainy,
            brainy_url: self.brainy_url.clone(),
            brainy_token: self.brainy_token.clone(),
            parent: self.parent.clone(),
            title: self.title.clone(),
            me: self.me.clone(),
        }
    }
}

/// Where the models and the language model come from.
#[derive(clap::Args, Clone)]
#[command(next_help_heading = "Without --server")]
struct LocalArgs {
    /// Folder of the GGUF files, used as it is; by default the pinned
    /// ones in ~/.cache/brainy-org, fetched from the mirror when missing.
    #[arg(long, global = true, env = "LOGBOOK_MODELS")]
    models: Option<PathBuf>,
    /// An OpenAI-compatible API, up to `/v1`, whose model cleans the final
    /// transcript up with --polish and writes the --report. The meeting's
    /// text goes there.
    #[arg(long, global = true, env = "LOGBOOK_POLISH_URL")]
    polish_url: Option<String>,
    #[arg(long, global = true, env = "LOGBOOK_POLISH_MODEL")]
    polish_model: Option<String>,
    #[arg(
        long,
        global = true,
        env = "LOGBOOK_POLISH_KEY",
        hide_env_values = true
    )]
    polish_key: Option<String>,
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
    /// The voices known by name, and the last meetings' speakers.
    Speakers {
        #[command(subcommand)]
        action: Option<SpeakersAction>,
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

#[derive(Subcommand)]
enum SpeakersAction {
    /// Name a speaker of a recent meeting: its voice is named on the
    /// pages of the meetings that follow.
    Name {
        /// The meeting (as `logbook speakers` lists it), or "last".
        meeting: String,
        /// Its label on the page ("Speaker 2"), or just its number.
        speaker: String,
        name: String,
    },
    /// Forget the voice of a person.
    Forget { name: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    // The library's messages (logbook_client) as plain lines on stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .without_time()
        .with_target(false)
        .with_level(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOGBOOK_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("logbook_client=info")),
        )
        .init();
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
    let backend = match &cli.server {
        Some(address) => Backend::Server(Server::new(address).with_token(cli.server_token)),
        None => Backend::Local(Arc::new(local::Local::new(local::Config {
            models: cli.local.models,
            polish_url: cli.local.polish_url,
            polish_model: cli.local.polish_model,
            polish_key: cli.local.polish_key,
        }))),
    };
    let server = &backend;
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
        } => {
            server.prepare(publish.polish || publish.report, 1).await?;
            play(server, &data, &file, speed, !no_diarize, &publish).await
        }
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
                (false, _) => audio::Echo::None,
                (true, true) => audio::Echo::Filter,
                (true, false) => audio::Echo::Cancel,
            };
            let gain = if no_mic { aec::Gain::Off } else { mic_gain };
            server.prepare(publish.polish || publish.report, 1).await?;
            live(server, &data, sources, echo, gain, &publish).await
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
                    meeting::finish(server, &mut pending, &brainy).await
                }
                .await;
                match result {
                    Ok(outcome) => print_outcome(&outcome),
                    Err(error) => {
                        eprintln!("  {error:#}");
                        failed += 1;
                    }
                }
            }
            if failed > 0 {
                bail!("{failed} meeting(s) still pending");
            }
            Ok(())
        }
        Command::Speakers { action } => match action {
            None => list_speakers(&data),
            Some(SpeakersAction::Name {
                meeting,
                speaker,
                name,
            }) => {
                let e = voices::name(&data, &meeting, &speaker, &name)?;
                eprintln!(
                    "{} of meeting {} is now {} ({:.0} s of voice; {} print(s) of {})",
                    e.label, e.meeting, e.name, e.seconds, e.prints, e.name
                );
                if e.weak() {
                    eprintln!(
                        "less than {:.0} s: {} may not be recognized every time; \
                         naming them in another meeting adds to it",
                        voices::GOOD_ENROLL,
                        e.name
                    );
                }
                Ok(())
            }
            Some(SpeakersAction::Forget { name }) => {
                voices::forget(&data, &name)?;
                eprintln!("{name}'s voice is forgotten");
                Ok(())
            }
        },
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
        } => {
            server.prepare(false, sessions).await?;
            bench(server, &file, sessions, stagger, !no_diarize).await
        }
    }
}

/// `logbook speakers`: the people known by voice, and the last meetings'
/// speakers that can be named.
fn list_speakers(data: &Path) -> Result<()> {
    let lib = voices::Library::load(data)?;
    if lib.people.is_empty() {
        println!("no voice known by name yet");
    } else {
        println!("known voices:");
        for p in &lib.people {
            let seconds: f64 = p.prints.iter().map(|q| q.seconds).sum();
            println!(
                "  {:<20} {} print(s), {:.0} s",
                p.name,
                p.prints.len(),
                seconds
            );
        }
    }
    let all = voices::meetings(data)?;
    if !all.is_empty() {
        println!("\nlast meetings (`logbook speakers name MEETING SPEAKER NAME`):");
        for (id, heard) in all.iter().rev() {
            let speakers: Vec<String> = heard
                .speakers
                .iter()
                .map(|s| format!("{} ({:.0} s)", s.label, s.voice.seconds))
                .collect();
            println!("  {id}  {}", speakers.join(", "));
        }
    }
    Ok(())
}

/// The final transcript and the report, when they went to no page.
fn print_outcome(outcome: &Outcome) {
    if let Some(transcript) = &outcome.transcript {
        println!("\n{transcript}");
    }
    if outcome.page.is_none()
        && let Some(report) = &outcome.report
    {
        println!("\n{report}");
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

/// Prints the live transcript.
struct Printer {
    start: Instant,
    speed: f64,
}

impl remote::Events for Printer {
    fn utterance(&mut self, u: Utterance) {
        println!(
            "[{}] {:<10} {}   (+{:.2} s)",
            clock(u.start),
            label(&u),
            u.text,
            latency(self.start, self.speed, &u)
        );
    }

    fn finalizing(&mut self, audio_s: f64, _id: &str) {
        eprintln!("finishing the final pass ({})…", clock(audio_s));
    }
}

async fn run_meeting(
    server: &Backend,
    data: &Path,
    channels: Vec<ChannelConfig>,
    p: &Publish,
    audio: remote::Audio,
    start: Instant,
    speed: f64,
) -> Result<()> {
    let mut printer = Printer { start, speed };
    let outcome = meeting::meeting(
        server,
        data,
        channels,
        &p.publish(),
        audio,
        meeting::Notes::default(),
        &mut printer,
    )
    .await?;
    if let Some(outcome) = outcome {
        print_outcome(&outcome);
    }
    Ok(())
}

async fn play(
    server: &Backend,
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
    run_meeting(server, data, channels, p, rx, start, speed).await
}

/// Each source is a channel: (what, which device, diarized).
async fn live(
    server: &Backend,
    data: &Path,
    sources: Vec<(audio::Source, Option<String>, bool)>,
    echo: audio::Echo,
    gain: aec::Gain,
    p: &Publish,
) -> Result<()> {
    let recording = audio::Recording::start(sources, echo, gain)?;
    for (channel, device) in &recording.devices {
        eprintln!("{channel}: {device}");
    }
    if echo == audio::Echo::Cancel {
        eprintln!("cancelling the call's echo from the microphone");
    }
    if gain != aec::Gain::Off {
        eprintln!("raising the microphone's level: {gain:?}");
    }
    eprintln!("recording; Ctrl-C to stop");
    let stop = Mutex::new(Some(recording.stop));
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("stopping…");
        stop.lock().unwrap().take();
    });
    run_meeting(
        server,
        data,
        recording.channels,
        p,
        recording.audio,
        recording.start,
        1.0,
    )
    .await
}

async fn bench(
    server: &Backend,
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
            let live = server.live(channels, None, rx, None, &mut events).await;
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

impl remote::Events for Latencies {
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
