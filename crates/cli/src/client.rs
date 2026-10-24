//! One session with logbook-server (docs/protocol.md): sends the channels,
//! then the audio as it comes, then `stop`; hands every utterance to the
//! caller until `done`.

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use logbook_engine::protocol::{
    ChannelConfig, ClientMessage, ServerMessage, Utterance, encode_audio,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Audio for the session: a channel index and 16 kHz mono samples. The
/// session stops when the sender is dropped.
pub type Audio = UnboundedReceiver<(u8, Vec<i16>)>;

pub async fn run(
    url: &str,
    channels: Vec<ChannelConfig>,
    mut audio: Audio,
    mut on_utterance: impl FnMut(Utterance),
) -> Result<()> {
    let (socket, _) = connect_async(url)
        .await
        .with_context(|| format!("could not connect to {url}"))?;
    let (mut sink, mut stream) = socket.split();
    let start = serde_json::to_string(&ClientMessage::Start { channels })?;
    sink.send(Message::Text(start.into())).await?;

    match next(&mut stream).await? {
        Some(ServerMessage::Ready) => {}
        Some(ServerMessage::Error { message }) => {
            bail!("the server refused the session: {message}")
        }
        other => bail!("expected ready, got {other:?}"),
    }

    let sender = tokio::spawn(async move {
        while let Some((channel, samples)) = audio.recv().await {
            sink.send(Message::Binary(encode_audio(channel, &samples).into()))
                .await?;
        }
        let stop = serde_json::to_string(&ClientMessage::Stop)?;
        sink.send(Message::Text(stop.into())).await?;
        anyhow::Ok(sink)
    });

    loop {
        match next(&mut stream).await? {
            Some(ServerMessage::Utterance(u)) => on_utterance(u),
            Some(ServerMessage::Done) => break,
            Some(ServerMessage::Error { message }) => bail!("server: {message}"),
            Some(ServerMessage::Ready) => {}
            None => bail!("the server closed the connection before done"),
        }
    }
    let mut sink = sender.await??;
    let _ = sink.close().await;
    Ok(())
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
