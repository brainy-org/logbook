//! Talking to logbook-server (docs/protocol.md).
//!
//! `live`: a session over the WebSocket that sends the channels, then the
//! audio as it comes, then `stop`, and hands every utterance to the caller.
//! A lost connection does not end it: the audio keeps coming (and being
//! kept by the caller), and a new session is opened as soon as the server
//! answers. With a final pass, the new session takes it up where its final
//! segments stopped: it is sent the audio kept from there, then the audio
//! as it comes (docs/protocol.md, "Reconnection"). `upload` and `poll`:
//! final passes over HTTP, the audio streamed from where the final
//! segments stopped; both try again for a while when the server does not
//! answer, then give up, and the caller keeps the meeting pending.
//! `report`: the meeting's report by the server's model.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::codec::StreamEncoder;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::protocol::{
    ChannelConfig, ClientMessage, FinalState, Report, ReportRequest, ServerMessage, SpeakerVoice,
    Started, Upload, Utterance, encode_audio,
};
use logbook_engine::recording::RangeReader;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// Audio for the session: a channel index and 16 kHz mono samples. The
/// session stops when the sender is dropped.
pub type Audio = UnboundedReceiver<(u8, Vec<i16>)>;

/// The first wait before opening a session again, doubled up to `RETRY_MAX`.
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// How often a running final pass is asked about.
const POLL: Duration = Duration::from_secs(1);
/// Failed requests in a row before polling gives up.
const POLL_FAILURES: u32 = 10;
/// Tries of an upload the server does not answer, `RETRY_MIN` apart and
/// doubling: about a minute before the meeting is left pending.
const UPLOAD_TRIES: u32 = 6;
/// Audio read from the kept files at once, to send again or to upload.
const KEPT_PART: u64 = 10 * SAMPLE_RATE as u64;

/// The server: `http://host:3300`; `ws://host:3300/live` is taken too.
/// With a person's token, sent on every request (made on the server's
/// admin page).
#[derive(Clone)]
pub struct Server {
    http: String,
    token: Option<String>,
}

impl Server {
    pub fn new(address: &str) -> Self {
        let address = address.trim_end_matches('/');
        let address = address.strip_suffix("/live").unwrap_or(address);
        let http = if let Some(rest) = address.strip_prefix("ws://") {
            format!("http://{rest}")
        } else if let Some(rest) = address.strip_prefix("wss://") {
            format!("https://{rest}")
        } else {
            address.to_string()
        };
        Self { http, token: None }
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        self
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn live(&self) -> String {
        let ws = if let Some(rest) = self.http.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.http.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.http.clone()
        };
        format!("{ws}/live")
    }
}

/// The final pass's result: every channel's segments by start, and what
/// was left out.
#[derive(Deserialize, Serialize)]
pub struct Finalized {
    pub utterances: Vec<Utterance>,
    pub warnings: Vec<String>,
    /// The speakers' voices at the end (voices.rs).
    #[serde(default)]
    pub voices: Vec<SpeakerVoice>,
}

pub struct Live {
    /// The final pass completed over the session: its warnings. Its
    /// segments went to `on_segments` as they came.
    pub finalized: Option<Vec<String>>,
    /// The final pass the server started, if it said so: its result is at
    /// `GET /finals/{id}` when the connection was lost before it came.
    pub final_id: Option<String>,
    /// The first session lasted from the first sample to `done`.
    pub intact: bool,
    /// Final segments starting before this have come; a session that
    /// takes the final pass up again starts here.
    pub final_until: f64,
    /// The meeting's speakers' voices as last sent.
    pub voices: Vec<SpeakerVoice>,
}

impl Live {
    pub fn new() -> Self {
        Self {
            finalized: None,
            final_id: None,
            intact: true,
            final_until: 0.0,
            voices: Vec::new(),
        }
    }
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

/// A channel's audio as the caller keeps it (pending.rs), on the meeting's
/// clock: read again to take a final pass up.
pub type Kept = Box<dyn Fn(usize) -> RangeReader + Send + Sync>;

/// Callbacks of a live session.
pub trait Events {
    fn utterance(&mut self, u: Utterance);
    /// Final segments starting before `until`, during the meeting and at
    /// its end (`until` infinite with the last ones), and every speaker's
    /// voice so far.
    fn final_segments(
        &mut self,
        _until: f64,
        _utterances: Vec<Utterance>,
        _voices: Vec<SpeakerVoice>,
    ) {
    }
    /// The meeting has ended; the rest of the final pass runs as `id`.
    fn finalizing(&mut self, _audio_s: f64, _id: &str) {}
}

/// With `final_pass`, the session asks for the final pass, which follows
/// the meeting (`Events::final_segments`); its value is whether the
/// server's model may clean it up. After a lost connection, a new session
/// takes the final pass up where its segments stopped when the audio is
/// `kept` (it is sent again from there); without, it has none: it would
/// not hear the whole meeting.
pub async fn live(
    server: &Server,
    channels: Vec<ChannelConfig>,
    final_pass: Option<bool>,
    mut audio: Audio,
    kept: Option<Kept>,
    events: &mut impl Events,
) -> Live {
    let url = server.live();
    let mut out = Live::new();
    // Samples taken from `audio` so far, per channel.
    let mut position = vec![0u64; channels.len()];
    // Per channel, the end of the last live utterance passed on: one heard
    // again (sent again to take the final pass up) is not passed twice.
    let mut shown = vec![f64::NEG_INFINITY; channels.len()];
    let mut ended = false;
    let mut first = true;
    // The kept audio could not be read back: later sessions have no final
    // pass (the one at the end reads what is there).
    let mut unreadable = false;
    let mut retry = RETRY_MIN;
    loop {
        let resume = !first && final_pass.is_some() && kept.is_some() && !unreadable;
        let asked = final_pass.filter(|_| first || resume);
        // A session taking the final pass up starts where its segments
        // stopped, on a sample, every channel there: the server puts its
        // times on the meeting's clock. Otherwise it starts now, and its
        // times are moved here, channel by channel.
        let from = if resume {
            ((out.final_until * SAMPLE_RATE as f64) as u64)
                .min(position.iter().copied().min().unwrap_or(0))
        } else {
            0
        };
        let start = Start {
            channels: &channels,
            final_pass: asked,
            offset: from as f64 / SAMPLE_RATE as f64,
            voices: if resume {
                out.voices.clone()
            } else {
                Vec::new()
            },
        };
        let lost = match connect(server, &url, &start).await {
            Ok(socket) => {
                if !first {
                    tracing::info!("live: connected again");
                }
                if resume {
                    tracing::info!("live: the final pass goes on from {:.0} s", start.offset);
                }
                retry = RETRY_MIN;
                let base: Vec<f64> = if first || resume {
                    vec![0.0; channels.len()]
                } else {
                    position
                        .iter()
                        .map(|&p| p as f64 / SAMPLE_RATE as f64)
                        .collect()
                };
                let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
                let shift = |u: Utterance| {
                    let by = names
                        .iter()
                        .position(|&n| n == u.channel)
                        .map_or(0.0, |i| base[i]);
                    u.shifted(by)
                };
                let replay = match (&kept, resume) {
                    (Some(kept), true) => Some(Replay {
                        readers: (0..channels.len()).map(kept).collect(),
                        next: vec![from; channels.len()],
                        end: position.clone(),
                    }),
                    _ => None,
                };
                let result = session(
                    socket,
                    &mut audio,
                    replay,
                    &mut unreadable,
                    &mut position,
                    &mut ended,
                    &mut out,
                    asked.is_some(),
                    |u: Utterance| {
                        let u = shift(u);
                        let i = names.iter().position(|&n| n == u.channel)?;
                        if u.end <= shown[i] {
                            return None;
                        }
                        shown[i] = u.end;
                        Some(u)
                    },
                    events,
                )
                .await;
                match result {
                    Ok(()) => return out,
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };
        first = false;
        out.intact = false;
        if ended {
            tracing::warn!("live: {lost:#}");
            return out;
        }
        tracing::warn!(
            "live: {lost:#}; still recording, trying again in {} s",
            retry.as_secs()
        );
        ended = drain(&mut audio, &mut position, retry).await;
        retry = (retry * 2).min(RETRY_MAX);
        if ended {
            return out;
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// What a session is opened with.
struct Start<'a> {
    channels: &'a [ChannelConfig],
    /// Whether the server's model may clean the final pass up, if asked.
    final_pass: Option<bool>,
    offset: f64,
    voices: Vec<SpeakerVoice>,
}

/// The kept audio a session is sent first: each channel from `next` to
/// `end` (samples), before the audio as it comes.
struct Replay {
    readers: Vec<RangeReader>,
    next: Vec<u64>,
    end: Vec<u64>,
}

impl Replay {
    /// The next part of the channel furthest behind, or None when all are
    /// sent: the channels go on together, as they were captured.
    fn next(&mut self) -> Result<Option<(u8, Vec<i16>)>> {
        let Some(i) = (0..self.next.len())
            .filter(|&i| self.next[i] < self.end[i])
            .min_by_key(|&i| self.next[i])
        else {
            return Ok(None);
        };
        let to = (self.next[i] + SAMPLE_RATE as u64).min(self.end[i]);
        let rate = SAMPLE_RATE as f64;
        let pcm = self.readers[i].read(self.next[i] as f64 / rate, to as f64 / rate)?;
        if pcm.is_empty() {
            bail!(
                "the audio kept ends before {:.0} s",
                self.next[i] as f64 / rate
            );
        }
        self.next[i] += pcm.len() as u64;
        Ok(Some((i as u8, to_i16(&pcm))))
    }
}

/// Samples read back from the kept files (which hold them as i16): exact.
fn to_i16(pcm: &[f32]) -> Vec<i16> {
    pcm.iter()
        .map(|&x| (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

async fn connect(server: &Server, url: &str, start: &Start<'_>) -> Result<Socket> {
    let mut request = url.into_client_request()?;
    if let Some(token) = &server.token {
        request
            .headers_mut()
            .insert(AUTHORIZATION, format!("Bearer {token}").parse()?);
    }
    let (mut socket, _) = match connect_async(request).await {
        Ok(connected) => connected,
        Err(tungstenite::Error::Http(response))
            if response.status() == tungstenite::http::StatusCode::UNAUTHORIZED =>
        {
            bail!(refused(server));
        }
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!("could not connect to {url}")));
        }
    };
    let start = serde_json::to_string(&ClientMessage::Start {
        channels: start.channels.to_vec(),
        final_pass: start.final_pass.is_some(),
        polish: start.final_pass.unwrap_or(false),
        offset: start.offset,
        voices: start.voices.clone(),
    })?;
    socket.send(Message::Text(start.into())).await?;
    match next(&mut socket).await? {
        Some(ServerMessage::Ready) => Ok(socket),
        Some(ServerMessage::Error { message }) => {
            bail!("the server refused the session: {message}")
        }
        other => bail!("expected ready, got {other:?}"),
    }
}

/// One session, until `done` (Ok) or until it is lost: the kept audio of
/// `replay` first, then the audio as it comes. `live` puts a live
/// utterance on the meeting's clock, or drops it; final segments come on
/// it.
#[allow(clippy::too_many_arguments)]
async fn session(
    socket: Socket,
    audio: &mut Audio,
    mut replay: Option<Replay>,
    unreadable: &mut bool,
    position: &mut [u64],
    ended: &mut bool,
    out: &mut Live,
    final_pass: bool,
    mut live: impl FnMut(Utterance) -> Option<Utterance>,
    events: &mut impl Events,
) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            // A part of the kept audio: always ready until all is sent.
            _ = std::future::ready(()), if replay.is_some() => {
                let part = replay.as_mut().expect("checked").next();
                if part.is_err() {
                    *unreadable = true;
                }
                match part? {
                    Some((channel, samples)) => {
                        sink.send(Message::Binary(encode_audio(channel, &samples).into()))
                            .await
                            .context("the connection was lost")?;
                    }
                    None => replay = None,
                }
            }
            frame = audio.recv(), if !*ended && replay.is_none() => match frame {
                Some((channel, samples)) => {
                    if let Some(p) = position.get_mut(channel as usize) {
                        *p += samples.len() as u64;
                    }
                    sink.send(Message::Binary(encode_audio(channel, &samples).into()))
                        .await
                        .context("the connection was lost")?;
                }
                None => {
                    *ended = true;
                    let stop = serde_json::to_string(&ClientMessage::Stop)?;
                    sink.send(Message::Text(stop.into()))
                        .await
                        .context("the connection was lost")?;
                }
            },
            message = next(&mut stream) => {
                let Some(message) = message.context("the connection was lost")? else {
                    bail!("the server closed the connection");
                };
                if handle(message, out, final_pass, &mut live, events)? {
                    let _ = sink.close().await;
                    return Ok(());
                }
            }
        }
    }
}

/// A message of the session, to `events` and `out`; true once it is done.
/// The local session (local.rs) hears the same messages.
pub fn handle(
    message: ServerMessage,
    out: &mut Live,
    final_pass: bool,
    mut live: impl FnMut(Utterance) -> Option<Utterance>,
    events: &mut impl Events,
) -> Result<bool> {
    match message {
        ServerMessage::Utterance(u) => {
            if let Some(u) = live(u) {
                events.utterance(u);
            }
        }
        ServerMessage::FinalSegments {
            until,
            utterances,
            voices,
        } if final_pass => {
            out.final_until = out.final_until.max(until);
            if !voices.is_empty() {
                out.voices = voices.clone();
            }
            events.final_segments(until, utterances, voices);
        }
        ServerMessage::FinalSegments { .. } => {}
        ServerMessage::Finalizing { audio_s, id } => {
            events.finalizing(audio_s, &id);
            out.final_id = Some(id);
        }
        ServerMessage::Final {
            utterances,
            warnings,
            voices,
        } => {
            out.final_until = f64::MAX;
            events.final_segments(f64::INFINITY, utterances, voices);
            out.finalized = Some(warnings);
        }
        ServerMessage::Done => return Ok(true),
        ServerMessage::Error { message } => bail!("server: {message}"),
        ServerMessage::Ready => {}
    }
    Ok(false)
}

/// Takes the audio without sending it for `wait`; true if it ended.
async fn drain(audio: &mut Audio, position: &mut [u64], wait: Duration) -> bool {
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return false,
            frame = audio.recv() => match frame {
                Some((channel, samples)) => {
                    if let Some(p) = position.get_mut(channel as usize) {
                        *p += samples.len() as u64;
                    }
                }
                None => return true,
            },
        }
    }
}

async fn next<S>(stream: &mut S) -> Result<Option<ServerMessage>>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = stream.next().await {
        match message? {
            Message::Text(text) => return Ok(Some(serde_json::from_str(text.as_str())?)),
            Message::Close(_) => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

fn http() -> Result<reqwest::Client> {
    // reqwest is built without a crypto provider (rustls on ring).
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(reqwest::Client::builder().build()?)
}

/// A channel kept here, to upload: `reader` from sample `from` to `to`
/// on the meeting's clock.
pub struct KeptChannel {
    pub reader: RangeReader,
    pub from: u64,
    pub to: u64,
}

/// Sends a meeting kept here for a final pass, from `offset` on the
/// meeting's clock (where its final segments stopped) with its speakers
/// before it; returns the pass's id. Each channel is read, encoded and
/// sent a part at a time.
pub async fn upload(
    server: &Server,
    channels: Vec<ChannelConfig>,
    polish: bool,
    offset: f64,
    voices: Vec<SpeakerVoice>,
    audio: Vec<KeptChannel>,
) -> Result<String> {
    let samples: u64 = audio.iter().map(|a| a.to.saturating_sub(a.from)).sum();
    tracing::info!(
        "uploading the meeting from {:.0} s ({:.0} s of audio, about {:.0} MB)…",
        offset,
        samples as f64 / SAMPLE_RATE as f64,
        samples as f64 / SAMPLE_RATE as f64 * 4000.0 / 1e6
    );
    let meeting = serde_json::to_vec(&Upload {
        channels,
        polish,
        offset,
        voices,
    })?;
    let audio: Vec<std::sync::Arc<KeptChannel>> =
        audio.into_iter().map(std::sync::Arc::new).collect();
    let url = format!("{}/finals", server.http);
    let client = http()?;
    let mut retry = RETRY_MIN;
    let mut tries = 1;
    let response = loop {
        // A form is consumed by its request: built again for each try.
        let mut form = reqwest::multipart::Form::new().part(
            "meeting",
            reqwest::multipart::Part::bytes(meeting.clone()).mime_str("application/json")?,
        );
        for channel in &audio {
            form = form.part(
                "audio",
                reqwest::multipart::Part::stream(encoded(channel.clone())),
            );
        }
        match server
            .authorized(client.post(&url))
            .multipart(form)
            .send()
            .await
        {
            Ok(response) => break response,
            // The server not up yet, or the network back in a moment.
            Err(error) if (error.is_connect() || error.is_timeout()) && tries < UPLOAD_TRIES => {
                tracing::warn!(
                    "could not reach {url}; trying again in {} s",
                    retry.as_secs()
                );
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(RETRY_MAX);
                tries += 1;
            }
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!("could not reach {url}")));
            }
        }
    };
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("the server refused the upload ({status}): {}", text.trim());
    }
    let started: Started = response.json().await?;
    Ok(started.id)
}

/// A channel's audio part (engine codec.rs), encoded on a thread of its
/// own as the request takes it: a few MB in memory at a time.
fn encoded(channel: std::sync::Arc<KeptChannel>) -> reqwest::Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    std::thread::spawn(move || {
        let run = || -> Result<()> {
            let (mut encoder, header) = StreamEncoder::new(channel.to - channel.from)?;
            if tx.blocking_send(Ok(header)).is_err() {
                return Ok(());
            }
            let rate = SAMPLE_RATE as f64;
            let mut at = channel.from;
            while at < channel.to {
                let to = (at + KEPT_PART).min(channel.to);
                let pcm = channel.reader.read(at as f64 / rate, to as f64 / rate)?;
                if pcm.is_empty() {
                    bail!("the audio kept ends before {:.0} s", at as f64 / rate);
                }
                at += pcm.len() as u64;
                let mut bytes = Vec::new();
                encoder.push(&to_i16(&pcm), &mut bytes)?;
                if tx.blocking_send(Ok(bytes)).is_err() {
                    return Ok(());
                }
            }
            let mut bytes = Vec::new();
            encoder.finish(&mut bytes)?;
            let _ = tx.blocking_send(Ok(bytes));
            Ok(())
        };
        if let Err(error) = run() {
            let _ = tx.blocking_send(Err(std::io::Error::other(format!("{error:#}"))));
        }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    reqwest::Body::wrap_stream(stream)
}

/// Whether the server answers and takes the token: a final pass it does
/// not know is a 404, a token it does not know a 401.
pub async fn check(server: &Server) -> Result<()> {
    let url = format!("{}/finals/check", server.http);
    let response = server
        .authorized(http()?.get(&url))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!(refused(server));
    }
    if response.status() != reqwest::StatusCode::NOT_FOUND {
        bail!(
            "{} answered {}: not a logbook-server",
            server.http,
            response.status()
        );
    }
    Ok(())
}

/// A token the server refuses, before a meeting starts; a server not
/// reached yet is no error (the live session tries again until it is).
pub async fn check_token(server: &Server) -> Result<()> {
    let url = format!("{}/finals/check", server.http);
    let answer = server
        .authorized(http()?.get(&url))
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    if let Ok(response) = answer
        && response.status() == reqwest::StatusCode::UNAUTHORIZED
    {
        bail!(refused(server));
    }
    Ok(())
}

/// The meeting's report, written by the server's model (`POST /reports`).
pub async fn report(server: &Server, request: &ReportRequest) -> Result<String> {
    let url = format!("{}/reports", server.http);
    let response = server
        .authorized(http()?.post(&url))
        .json(request)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_IMPLEMENTED {
        bail!("the server has no language model to write the report");
    }
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!(
            "the server could not write the report ({status}): {}",
            text.trim()
        );
    }
    let report: Report = response.json().await?;
    Ok(report.markdown)
}

pub enum Pass {
    Done(Finalized),
    /// The server does not know the pass (a restart, or a result kept past
    /// its time): upload the meeting again.
    Unknown,
    Failed(String),
}

/// Waits for the final pass `id`; an error if the server cannot be reached.
pub async fn poll(server: &Server, id: &str) -> Result<Pass> {
    let url = format!("{}/finals/{id}", server.http);
    let client = http()?;
    let mut failures = 0;
    loop {
        let answer = async {
            let response = server.authorized(client.get(&url)).send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let state: FinalState = response.error_for_status()?.json().await?;
            anyhow::Ok(Some(state))
        }
        .await;
        match answer {
            Ok(None) => return Ok(Pass::Unknown),
            Ok(Some(FinalState::Done {
                utterances,
                warnings,
                voices,
            })) => {
                return Ok(Pass::Done(Finalized {
                    utterances,
                    warnings,
                    voices,
                }));
            }
            Ok(Some(FinalState::Failed { message })) => return Ok(Pass::Failed(message)),
            Ok(Some(FinalState::Running { .. })) => failures = 0,
            Err(error) => {
                failures += 1;
                if failures >= POLL_FAILURES {
                    return Err(error.context(format!("could not reach {url}")));
                }
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

/// What a 401 means to the person.
fn refused(server: &Server) -> String {
    match server.token {
        Some(_) => format!("{} refused the token", server.http),
        None => format!("{} needs a token (made on its admin page)", server.http),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses() {
        for address in [
            "http://h:3300",
            "http://h:3300/",
            "ws://h:3300/live",
            "ws://h:3300",
        ] {
            let server = Server::new(address);
            assert_eq!(server.http, "http://h:3300", "{address}");
            assert_eq!(server.live(), "ws://h:3300/live");
        }
        assert_eq!(Server::new("wss://h/live").http, "https://h");
        assert_eq!(Server::new("https://h").live(), "wss://h/live");
    }
}
