//! The final transcript cleaned up by a language model, as Wispr Flow does
//! for dictation (docs/pipeline.md, "Clean-up";
//! docs/decisions/2026-10-25-llm-clean-up-of-the-final-transcript.md).
//!
//! Segments go to an OpenAI-compatible `/chat/completions` endpoint
//! (Ollama, llama.cpp's server, vLLM, a hosted provider) a minute or so at a
//! time, with the last ones already cleaned as context. The model returns
//! each segment's text without fillers, false starts and the words a
//! speaker took back, punctuated. Only the text changes: speakers, times
//! and the words as heard stay, so `text` may no longer be the words
//! joined. A text that grew, shrank more than fillers explain, or is
//! mostly words not heard in that segment (a neighbour's text, copied: a
//! small model did it) is the model writing rather than cleaning, and the
//! segment keeps what was heard; so does every segment of a part the
//! model could not do.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::protocol::Utterance;

/// The most text sent in one request: about a minute and a half of speech.
const MAX_CHARS: usize = 3000;
/// Segments already cleaned sent before a part, for its names and its
/// sentence.
pub const CONTEXT: usize = 3;
/// A cleaned text this much longer than the heard one (plus `SLACK`
/// characters, for punctuation) has words the speaker did not say.
const MAX_GROWTH: f64 = 1.25;
const SLACK: usize = 15;
/// A cleaned text shorter than this share of the heard one, past
/// `FEW_WORDS`, is a summary, not a clean-up.
const MIN_SHARE: f64 = 0.4;
/// The least share of a cleaned text's words heard in its segment (or
/// nearly: `similar`): a misheard word fixed here and there passes, a
/// text of its own or another segment's does not.
const MIN_HEARD: f64 = 0.75;
/// A segment of this many words or fewer may be all fillers ("Ehm, sì."
/// may not; "Ehm." is): the model may empty it, and it is dropped.
const FEW_WORDS: usize = 3;

const INSTRUCTIONS: &str = "\
You clean up the transcript of a meeting made by speech recognition, \
segment by segment, as a careful human editor would for the minutes.

For each segment:
- Remove filler words and hesitations (eh, ehm, uhm, cioè or diciamo used as filler), \
false starts, stutters and words repeated by mistake.
- When the speaker corrects themselves (\"Tuesday, no, Wednesday\"), keep only the correction.
- Fix punctuation and capital letters.
- Fix a word only when it was clearly misheard and the context makes the intended word certain.
- Keep the language the segment was spoken in, the speaker's words and their meaning: \
never translate, summarise, explain, reorder or add anything.
- A segment that is only fillers becomes an empty string.

The segments under \"context\" are earlier ones, already cleaned: read them, do not return them.
Answer with JSON only, every id of \"segments\" once, in this shape:
{\"segments\": [{\"id\": 0, \"text\": \"...\"}]}";

/// Where the model is and how to ask it.
#[derive(Clone, Debug)]
pub struct Options {
    /// The API's base, up to `/v1`: `http://localhost:11434/v1` for Ollama.
    pub url: String,
    pub model: String,
    /// Sent as a bearer token; none for a local server.
    pub key: Option<String>,
    /// For one request.
    pub timeout: Duration,
}

/// A transcript cleaned up: its segments, and what could not be done.
pub struct Polished {
    pub utterances: Vec<Utterance>,
    /// Parts the model could not clean, said as observed: they are as heard.
    pub errors: Vec<String>,
}

pub struct Polisher {
    options: Options,
    agent: ureq::Agent,
}

impl Polisher {
    pub fn new(options: Options) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(options.timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self { options, agent }
    }

    /// `utterances` cleaned in parts of up to `MAX_CHARS`, each after the
    /// last segments cleaned (`context` before the first). Blocks for a
    /// request per part: run it off the sessions' threads.
    pub fn polish(&self, context: &[Utterance], utterances: Vec<Utterance>) -> Polished {
        let mut done: Vec<Utterance> = Vec::with_capacity(utterances.len());
        let mut errors = Vec::new();
        for part in parts(utterances) {
            let before: Vec<&Utterance> = if done.is_empty() {
                last(context.iter())
            } else {
                last(done.iter())
            };
            let (from, to) = (part[0].start, part[part.len() - 1].end);
            match self.ask(&before, &part) {
                Ok(answer) => done.extend(apply(part, &answer)),
                Err(error) => {
                    errors.push(format!("{from:.0}–{to:.0} s: {error:#}"));
                    done.extend(part);
                }
            }
        }
        Polished {
            utterances: done,
            errors,
        }
    }

    /// The model's answer for one part: its message's text.
    fn ask(&self, context: &[&Utterance], part: &[Utterance]) -> Result<String> {
        let url = format!(
            "{}/chat/completions",
            self.options.url.trim_end_matches('/')
        );
        let mut request = self.agent.post(&url);
        if let Some(key) = &self.options.key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(body(&self.options.model, context, part))
            .with_context(|| format!("could not reach {url}"))?;
        let status = response.status();
        let text = response.body_mut().read_to_string()?;
        if !status.is_success() {
            bail!("{url} answered {status}: {}", text.trim());
        }
        let completion: Completion =
            serde_json::from_str(&text).context("not a chat completion")?;
        completion
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| anyhow!("the completion has no message"))
    }
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Answer {
    segments: Vec<Cleaned>,
}

#[derive(Deserialize)]
struct Cleaned {
    id: usize,
    text: String,
}

fn last<'a>(utterances: impl DoubleEndedIterator<Item = &'a Utterance>) -> Vec<&'a Utterance> {
    let mut out: Vec<&Utterance> = utterances.rev().take(CONTEXT).collect();
    out.reverse();
    out
}

/// Consecutive segments of up to `MAX_CHARS` of text (one longer segment
/// alone).
fn parts(utterances: Vec<Utterance>) -> Vec<Vec<Utterance>> {
    let mut out: Vec<Vec<Utterance>> = Vec::new();
    let mut chars = 0;
    for u in utterances {
        let n = u.text.chars().count();
        match out.last_mut() {
            Some(part) if chars + n <= MAX_CHARS => part.push(u),
            _ => {
                chars = 0;
                out.push(vec![u]);
            }
        }
        chars += n;
    }
    out
}

/// "mic", or "system 2" for the third speaker of the call.
fn speaker(u: &Utterance) -> String {
    match u.speaker {
        Some(s) => format!("{} {s}", u.channel),
        None => u.channel.clone(),
    }
}

fn body(model: &str, context: &[&Utterance], part: &[Utterance]) -> Value {
    let transcript = json!({
        "context": context
            .iter()
            .map(|u| json!({"speaker": speaker(u), "text": u.text}))
            .collect::<Vec<_>>(),
        "segments": part
            .iter()
            .enumerate()
            .map(|(id, u)| json!({"id": id, "speaker": speaker(u), "text": u.text}))
            .collect::<Vec<_>>(),
    });
    json!({
        "model": model,
        "temperature": 0,
        "messages": [
            {"role": "system", "content": INSTRUCTIONS},
            {"role": "user", "content": transcript.to_string()},
        ],
    })
}

/// The part with the texts of the answer, where they pass `accept`; as
/// heard where they do not, or the answer cannot be read.
fn apply(part: Vec<Utterance>, answer: &str) -> Vec<Utterance> {
    let Some(answer) = read(answer) else {
        return part;
    };
    let mut texts: Vec<Option<String>> = vec![None; part.len()];
    for c in answer.segments {
        if let Some(slot) = texts.get_mut(c.id) {
            *slot = Some(c.text.trim().to_string());
        }
    }
    part.into_iter()
        .zip(texts)
        .filter_map(|(mut u, text)| match text {
            Some(t) if t.is_empty() => (u.words.len() > FEW_WORDS).then_some(u),
            Some(t) if accept(&u, &t) => {
                u.text = t;
                Some(u)
            }
            _ => Some(u),
        })
        .collect()
}

/// The answer's JSON, past what models put around it: a `<think>` block,
/// a code fence, a sentence.
fn read(answer: &str) -> Option<Answer> {
    let answer = match answer.rfind("</think>") {
        Some(i) => &answer[i + "</think>".len()..],
        None => answer,
    };
    let (from, to) = (answer.find('{')?, answer.rfind('}')?);
    serde_json::from_str(answer.get(from..=to)?).ok()
}

/// A clean-up, not a rewrite: no longer than the heard text allows, and
/// not much shorter unless it was short.
fn accept(heard: &Utterance, cleaned: &str) -> bool {
    let (before, after) = (heard.text.chars().count(), cleaned.chars().count());
    if after as f64 > before as f64 * MAX_GROWTH + SLACK as f64 {
        return false;
    }
    if heard.words.len() > FEW_WORDS * 2 && (after as f64) < before as f64 * MIN_SHARE {
        return false;
    }
    let heard = tokens(&heard.text);
    let cleaned = tokens(cleaned);
    let kept = cleaned
        .iter()
        .filter(|c| heard.iter().any(|h| similar(h, c)))
        .count();
    kept as f64 >= cleaned.len() as f64 * MIN_HEARD
}

/// Words compared: lowercase, without punctuation.
fn tokens(text: &str) -> Vec<Vec<char>> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase().chars().collect())
        .collect()
}

/// One word, or one with a letter or two of its own changed ("Anno",
/// "Hanno"): at most a third of the longer one edited.
fn similar(a: &[char], b: &[char]) -> bool {
    a == b || edits(a, b) * 3 <= a.len().max(b.len())
}

/// Levenshtein distance.
fn edits(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diagonal + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;
    use crate::protocol::Word;

    fn u(start: f64, text: &str) -> Utterance {
        Utterance {
            channel: "system".into(),
            speaker: Some(0),
            start,
            end: start + 1.0,
            text: text.into(),
            words: text
                .split(' ')
                .map(|w| Word {
                    w: w.into(),
                    start,
                    end: start + 0.1,
                    conf: 1.0,
                })
                .collect(),
        }
    }

    #[test]
    fn texts_are_replaced_and_words_kept() {
        let part = vec![
            u(0.0, "ehm allora ci vediamo martedì no mercoledì"),
            u(2.0, "Ehm."),
        ];
        let answer =
            r#"{"segments":[{"id":0,"text":"Allora ci vediamo mercoledì."},{"id":1,"text":""}]}"#;
        let out = apply(part, answer);
        assert_eq!(out.len(), 1, "a segment of fillers alone is dropped");
        assert_eq!(out[0].text, "Allora ci vediamo mercoledì.");
        assert_eq!(out[0].words.len(), 7, "the words stay as heard");
    }

    #[test]
    fn a_rewrite_keeps_what_was_heard() {
        let heard = "quindi il budget lo chiudiamo entro venerdì e poi vediamo";
        let part = vec![u(0.0, heard), u(2.0, "sì perfetto grazie a tutti per oggi")];
        // Longer: words added. Much shorter: a summary. Empty: content gone.
        let answer = r#"<think>ok</think>```json
{"segments":[{"id":0,"text":"Quindi il budget, che è stato discusso a lungo nella riunione precedente, lo chiudiamo entro venerdì."},{"id":1,"text":""}]}
```"#;
        let out = apply(part, answer);
        assert_eq!(out[0].text, heard);
        assert_eq!(out[1].text, "sì perfetto grazie a tutti per oggi");
        let part = vec![u(0.0, heard)];
        let out = apply(part, r#"{"segments":[{"id":0,"text":"Budget venerdì."}]}"#);
        assert_eq!(out[0].text, heard);
    }

    #[test]
    fn a_neighbours_text_is_refused_and_a_fixed_word_is_not() {
        // LFM2.5-8B-A1B gave the third segment the second's text.
        let part = vec![
            u(0.0, "Sì, li chiamo oggi pomeriggio."),
            u(
                2.0,
                "Io preparo la bozza del contratto, con legge italiana al Foro di Genova.",
            ),
            u(
                5.0,
                "Anno la centrale operativa a Genova, il buggito range.",
            ),
        ];
        let answer = r#"{"segments":[
            {"id":0,"text":"Sì, li chiamo oggi pomeriggio."},
            {"id":1,"text":"Sì, li chiamo oggi pomeriggio."},
            {"id":2,"text":"Hanno la centrale operativa a Genova, il budget regge."}]}"#;
        let out = apply(part, answer);
        assert_eq!(
            out[1].text,
            "Io preparo la bozza del contratto, con legge italiana al Foro di Genova."
        );
        assert_eq!(
            out[2].text,
            "Hanno la centrale operativa a Genova, il budget regge."
        );
    }

    #[test]
    fn edits_are_counted() {
        let w = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(edits(&w("anno"), &w("hanno")), 1);
        assert_eq!(edits(&w("buggito"), &w("budget")), 3);
        assert!(similar(&w("anno"), &w("hanno")));
        assert!(!similar(&w("li"), &w("la")));
    }

    #[test]
    fn an_answer_that_cannot_be_read_changes_nothing() {
        let out = apply(vec![u(0.0, "ehm sì")], "I cannot help with that.");
        assert_eq!(out[0].text, "ehm sì");
        // Ids it left out, or made up, are ignored.
        let part = vec![u(0.0, "ehm sì"), u(1.0, "no")];
        let out = apply(
            part,
            r#"{"segments":[{"id":1,"text":"No."},{"id":7,"text":"x"}]}"#,
        );
        assert_eq!(
            (out[0].text.as_str(), out[1].text.as_str()),
            ("ehm sì", "No.")
        );
    }

    #[test]
    fn parts_hold_at_most_max_chars() {
        let long = "parola ".repeat(200);
        let utterances: Vec<Utterance> = (0..5).map(|i| u(i as f64, long.trim())).collect();
        let sizes: Vec<usize> = parts(utterances).iter().map(Vec::len).collect();
        assert_eq!(sizes, [2, 2, 1]);
    }

    #[test]
    fn the_request_carries_the_context_and_the_speakers() {
        let before = u(0.0, "Buongiorno a tutti.");
        let mut mic = u(1.0, "ehm buongiorno");
        mic.channel = "mic".into();
        mic.speaker = None;
        let body = body("m", &[&before], &[mic]);
        let content: Value =
            serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["context"][0]["speaker"], "system 0");
        assert_eq!(content["segments"][0]["speaker"], "mic");
        assert_eq!(content["segments"][0]["id"], 0);
    }

    /// One HTTP exchange with a canned answer; the request it read.
    fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then content-length bytes of body.
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_string)
                        })
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    fn polisher(url: String) -> Polisher {
        Polisher::new(Options {
            url,
            model: "test".into(),
            key: Some("secret".into()),
            timeout: Duration::from_secs(5),
        })
    }

    #[test]
    fn a_part_is_cleaned_through_the_endpoint() {
        let content = r#"{"segments":[{"id":0,"text":"Sì, va bene."}]}"#;
        let completion = json!({"choices": [{"message": {"content": content}}]}).to_string();
        let (url, server) = serve("200 OK", &completion);
        let out = polisher(url).polish(&[], vec![u(0.0, "ehm sì sì va bene")]);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.utterances[0].text, "Sì, va bene.");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions"));
        assert!(request.contains("Bearer secret"));
    }

    #[test]
    fn a_failed_request_leaves_the_part_as_heard() {
        let (url, server) = serve("500 Internal Server Error", r#"{"error":"no model"}"#);
        let out = polisher(url).polish(&[], vec![u(0.0, "ehm sì")]);
        server.join().unwrap();
        assert_eq!(out.utterances[0].text, "ehm sì");
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].contains("500"), "{}", out.errors[0]);
    }
}
