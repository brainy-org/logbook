//! The live session's codec end to end: `remote::live` against a fake
//! logbook-server on 127.0.0.1 that reads the binary frames as the real
//! one does (`AudioIn`). Whatever the two ends agree on (Opus, or PCM
//! with a server that answers a bare `ready`, or a client that asks for
//! nothing), each channel must arrive with exactly the samples sent, in
//! frames of uneven sizes, the last ones flushed before `stop`.

// Two channels, sent and heard, compared by index.
#![allow(clippy::needless_range_loop)]

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use logbook_client::remote::{self, Server};
use logbook_engine::codec;
use logbook_engine::protocol::{AudioIn, ChannelConfig, ClientMessage, Codec, ServerMessage};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// What the fake server was asked and heard.
#[derive(Default)]
struct Heard {
    asked: Option<(Codec, Option<u16>)>,
    samples: [Vec<f32>; 2],
    owed_at_stop: u64,
}

/// One session: `start`, `ready` (with the codec if `takes_opus` and the
/// client asked for it, bare otherwise), the frames, `stop`, `done`.
async fn fake_server(listener: TcpListener, takes_opus: bool, heard: Arc<Mutex<Heard>>) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    let Some(Ok(Message::Text(start))) = socket.next().await else {
        panic!("no start");
    };
    let ClientMessage::Start {
        codec, lookahead, ..
    } = serde_json::from_str(start.as_str()).unwrap()
    else {
        panic!("not a start");
    };
    heard.lock().unwrap().asked = Some((codec, lookahead));
    let answer = if takes_opus {
        Codec::answer(codec, lookahead)
    } else {
        None
    };
    let mut audio = AudioIn::new(answer, lookahead, 2).unwrap();
    let send = |m: &ServerMessage| Message::Text(serde_json::to_string(m).unwrap().into());
    socket
        .send(send(&ServerMessage::Ready { codec: answer }))
        .await
        .unwrap();
    while let Some(message) = socket.next().await {
        match message.unwrap() {
            Message::Binary(frame) => {
                if let Some((channel, pcm)) = audio.frame(&frame).unwrap() {
                    heard.lock().unwrap().samples[channel].extend(pcm);
                }
            }
            Message::Text(_) => {
                heard.lock().unwrap().owed_at_stop = audio.owed();
                socket.send(send(&ServerMessage::Done)).await.unwrap();
                break;
            }
            _ => {}
        }
    }
}

struct Quiet;
impl remote::Events for Quiet {
    fn utterance(&mut self, _: logbook_engine::protocol::Utterance) {}
}

/// Two channels of a tone, sent in frames of uneven sizes (a 100 ms
/// frame, a gap the `Timeline` fills, a few samples): what the server got.
async fn run(client: Codec, takes_opus: bool) -> (Heard, [Vec<i16>; 2]) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let heard = Arc::new(Mutex::new(Heard::default()));
    let server = tokio::spawn(fake_server(listener, takes_opus, heard.clone()));
    let sent: [Vec<i16>; 2] = std::array::from_fn(|c| {
        (0..23_457)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                ((t * (300.0 + 200.0 * c as f32) * std::f32::consts::TAU).sin() * 9_000.0) as i16
            })
            .collect()
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let sizes = [1600, 4813, 7, 1600, 333];
    let mut at = [0usize; 2];
    let mut k = 0;
    while at.iter().zip(&sent).any(|(a, s)| *a < s.len()) {
        for c in 0..2 {
            let to = (at[c] + sizes[(k + c) % sizes.len()]).min(sent[c].len());
            if at[c] < to {
                tx.send((c as u8, sent[c][at[c]..to].to_vec())).unwrap();
            }
            at[c] = to;
        }
        k += 1;
    }
    drop(tx);
    let channels = ["mic", "system"]
        .map(|name| ChannelConfig {
            name: name.into(),
            diarize: false,
            echo_of: None,
        })
        .to_vec();
    let server_address = Server::new(&format!("http://{address}")).with_codec(client);
    let live = remote::live(&server_address, channels, false, rx, None, &mut Quiet).await;
    assert!(live.intact);
    server.await.unwrap();
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    (heard, sent)
}

fn as_float(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|&s| s as f32 / 32768.0).collect()
}

#[tokio::test]
async fn a_new_client_and_a_new_server_send_opus_sample_for_sample() {
    let (heard, sent) = run(Codec::Opus, true).await;
    let (codec, lookahead) = heard.asked.unwrap();
    assert_eq!(codec, Codec::Opus);
    assert!(lookahead.is_some());
    assert_eq!(heard.owed_at_stop, 0, "the encoders flushed before stop");
    for c in 0..2 {
        assert_eq!(heard.samples[c].len(), sent[c].len(), "channel {c}");
        // Opus, not PCM: what the upload's codec makes of the same samples.
        let expected = codec::decode(&codec::encode(&sent[c]).unwrap()).unwrap();
        assert_eq!(heard.samples[c], expected, "channel {c}");
        assert_ne!(heard.samples[c], as_float(&sent[c]));
    }
}

#[tokio::test]
async fn a_new_client_and_an_older_server_send_pcm() {
    // The client asks for Opus; the server answers a bare ready.
    let (heard, sent) = run(Codec::Opus, false).await;
    assert_eq!(heard.asked.unwrap().0, Codec::Opus);
    for c in 0..2 {
        assert_eq!(heard.samples[c], as_float(&sent[c]), "channel {c}");
    }
}

#[tokio::test]
async fn a_client_that_asks_nothing_sends_pcm_to_a_new_server() {
    let (heard, sent) = run(Codec::Pcm, true).await;
    assert_eq!(heard.asked.unwrap(), (Codec::Pcm, None));
    for c in 0..2 {
        assert_eq!(heard.samples[c], as_float(&sent[c]), "channel {c}");
    }
}
