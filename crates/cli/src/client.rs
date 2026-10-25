//! Talking to logbook-server (docs/protocol.md).
//!
//! `live`: a session over the WebSocket that sends the channels, then the
//! audio as it comes, then `stop`, and hands every utterance to the caller.
//! A lost connection does not end it: the audio keeps coming (and being
//! recorded by the caller), and a new session is opened as soon as the
//! server answers; what was said in between has no live text, the final
//! pass covers it. `upload` and `poll`: final passes over HTTP.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::codec;
use logbook_engine::parakeet::SAMPLE_RATE;
use logbook_engine::protocol::{
    ChannelConfig, ClientMessage, FinalState, ServerMessage, Started, Upload, Utterance,
    encode_audio,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;
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

/// The server: `http://host:3300`; `ws://host:3300/live` is taken too.
#[derive(Clone)]
pub struct Server {
    http: String,
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
        Self { http }
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
}

/// Callbacks of a live session.
pub trait Events {
    fn utterance(&mut self, u: Utterance);
    /// Final segments starting before `until`, during the meeting and at
    /// its end (`until` infinite with the last ones).
    fn final_segments(&mut self, _until: f64, _utterances: Vec<Utterance>) {}
    /// The meeting has ended; the rest of the final pass runs as `id`.
    fn finalizing(&mut self, _audio_s: f64, _id: &str) {}
}

/// With `final_pass`, the first session asks for the final pass, which
/// follows the meeting (`Events::final_segments`). Only the first: a later
/// one did not hear the whole meeting.
pub async fn live(
    server: &Server,
    channels: Vec<ChannelConfig>,
    final_pass: bool,
    mut audio: Audio,
    events: &mut impl Events,
) -> Live {
    let url = server.live();
    let mut out = Live {
        finalized: None,
        final_id: None,
        intact: true,
    };
    // Samples taken from `audio` so far, per channel: where a new session's
    // sample 0 is on the meeting's clock.
    let mut position = vec![0usize; channels.len()];
    let mut ended = false;
    let mut first = true;
    let mut retry = RETRY_MIN;
    loop {
        let asked = final_pass && first;
        let lost = match connect(&url, &channels, asked).await {
            Ok(socket) => {
                if !first {
                    eprintln!("live: connected again");
                }
                retry = RETRY_MIN;
                let base = position.clone();
                let shift = |mut u: Utterance| {
                    let i = channels.iter().position(|c| c.name == u.channel);
                    let offset = i.map_or(0.0, |i| base[i] as f64 / SAMPLE_RATE as f64);
                    u.start += offset;
                    u.end += offset;
                    for w in &mut u.words {
                        w.start += offset;
                        w.end += offset;
                    }
                    u
                };
                let result = session(
                    socket,
                    &mut audio,
                    &mut position,
                    &mut ended,
                    &mut out,
                    asked,
                    shift,
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
            eprintln!("live: {lost:#}");
            return out;
        }
        eprintln!(
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

async fn connect(url: &str, channels: &[ChannelConfig], final_pass: bool) -> Result<Socket> {
    let (mut socket, _) = connect_async(url)
        .await
        .with_context(|| format!("could not connect to {url}"))?;
    let start = serde_json::to_string(&ClientMessage::Start {
        channels: channels.to_vec(),
        final_pass,
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

/// One session, until `done` (Ok) or until it is lost. `shift` puts a
/// later session's times on the meeting's clock; final segments only come
/// from the first, which is on it.
#[allow(clippy::too_many_arguments)]
async fn session(
    socket: Socket,
    audio: &mut Audio,
    position: &mut [usize],
    ended: &mut bool,
    out: &mut Live,
    final_pass: bool,
    shift: impl Fn(Utterance) -> Utterance,
    events: &mut impl Events,
) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            frame = audio.recv(), if !*ended => match frame {
                Some((channel, samples)) => {
                    if let Some(p) = position.get_mut(channel as usize) {
                        *p += samples.len();
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
            message = next(&mut stream) => match message.context("the connection was lost")? {
                Some(ServerMessage::Utterance(u)) => events.utterance(shift(u)),
                Some(ServerMessage::FinalSegments { until, utterances }) if final_pass => {
                    events.final_segments(until, utterances);
                }
                Some(ServerMessage::FinalSegments { .. }) => {}
                Some(ServerMessage::Finalizing { audio_s, id }) => {
                    events.finalizing(audio_s, &id);
                    out.final_id = Some(id);
                }
                Some(ServerMessage::Final { utterances, warnings }) => {
                    events.final_segments(f64::INFINITY, utterances);
                    out.finalized = Some(warnings);
                }
                Some(ServerMessage::Done) => {
                    let _ = sink.close().await;
                    return Ok(());
                }
                Some(ServerMessage::Error { message }) => bail!("server: {message}"),
                Some(ServerMessage::Ready) => {}
                None => bail!("the server closed the connection"),
            },
        }
    }
}

/// Takes the audio without sending it for `wait`; true if it ended.
async fn drain(audio: &mut Audio, position: &mut [usize], wait: Duration) -> bool {
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return false,
            frame = audio.recv() => match frame {
                Some((channel, samples)) => {
                    if let Some(p) = position.get_mut(channel as usize) {
                        *p += samples.len();
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

/// Sends a meeting recorded here for a final pass; returns the pass's id.
pub async fn upload(
    server: &Server,
    channels: Vec<ChannelConfig>,
    audio: Vec<Vec<i16>>,
) -> Result<String> {
    // Encoding an hour takes a few seconds of CPU.
    let encoded = tokio::task::spawn_blocking(move || {
        audio
            .iter()
            .map(|pcm| codec::encode(pcm))
            .collect::<Result<Vec<_>>>()
    })
    .await??;
    let bytes: usize = encoded.iter().map(Vec::len).sum();
    eprintln!("uploading the meeting ({:.1} MB)…", bytes as f64 / 1e6);
    let meeting = serde_json::to_vec(&Upload { channels })?;
    let mut form = reqwest::multipart::Form::new().part(
        "meeting",
        reqwest::multipart::Part::bytes(meeting).mime_str("application/json")?,
    );
    for data in encoded {
        form = form.part("audio", reqwest::multipart::Part::bytes(data));
    }
    let url = format!("{}/finals", server.http);
    let response = http()?
        .post(&url)
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("the server refused the upload ({status}): {}", text.trim());
    }
    let started: Started = response.json().await?;
    Ok(started.id)
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
            let response = client.get(&url).send().await?;
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
            })) => {
                return Ok(Pass::Done(Finalized {
                    utterances,
                    warnings,
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
