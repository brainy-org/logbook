//! The language model that writes the meeting's report (report.rs): an
//! OpenAI-compatible endpoint (Ollama, llama.cpp's server, vLLM, a hosted
//! provider), its chat completions and the models it lists.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

/// Seconds a request may take by default.
pub const TIMEOUT: u64 = 120;

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

/// The HTTP client of model requests (ureq's).
pub type Agent = ureq::Agent;

/// An agent for `options`: its timeout, statuses read as answers.
pub fn agent(options: &Options) -> Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(options.timeout))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Tokens a model may write for one answer, its reasoning included. Left
/// to the provider, Groq stops gpt-oss at 2048: on a 30-minute meeting's
/// report it reasoned until then half the time and wrote nothing.
pub const MAX_ANSWER_TOKENS: u32 = 8192;

/// Tries of a request the provider refuses for its rate.
const RATE_TRIES: u32 = 3;
/// The longest wait for the rate, whatever the provider says.
const MAX_RATE_WAIT: Duration = Duration::from_secs(70);
/// The wait when the provider does not say.
const RATE_WAIT: Duration = Duration::from_secs(20);

/// How long the provider says to wait (`retry-after`, in seconds).
fn rate_wait(response: &ureq::http::Response<ureq::Body>) -> Duration {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|s| Duration::from_secs_f64(s.max(1.0) + 1.0))
        .unwrap_or(RATE_WAIT)
        .min(MAX_RATE_WAIT)
}

/// One chat completion at `options.url`: the first choice's text. The
/// answer's limit (`MAX_ANSWER_TOKENS`) is added to `body`.
pub fn complete(agent: &Agent, options: &Options, mut body: Value) -> Result<String> {
    let url = format!("{}/chat/completions", options.url.trim_end_matches('/'));
    // OpenAI's reasoning models refuse `max_tokens`; Mistral refuses any
    // field it does not know, `max_completion_tokens` among them.
    let limit = if url.contains("api.mistral.ai") {
        "max_tokens"
    } else {
        "max_completion_tokens"
    };
    body[limit] = MAX_ANSWER_TOKENS.into();
    let mut tries = 1;
    let text = loop {
        let mut request = agent.post(&url);
        if let Some(key) = &options.key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = request
            .send_json(&body)
            .with_context(|| format!("could not reach {url}"))?;
        let status = response.status();
        let wait = rate_wait(&response);
        let text = response.body_mut().read_to_string()?;
        if status.is_success() {
            break text;
        }
        // Over the provider's rate (Groq's free tier: 8000 tokens a
        // minute, about one request of a meeting): the wait it says.
        let limited = status == 429 || (status == 413 && text.contains("rate_limit_exceeded"));
        if limited && tries < RATE_TRIES {
            warn!(
                "{url} answered {status}: over its rate; trying again in {} s",
                wait.as_secs()
            );
            std::thread::sleep(wait);
            tries += 1;
            continue;
        }
        bail!("{url} answered {status}: {}", text.trim());
    };
    let completion: Completion = serde_json::from_str(&text).context("not a chat completion")?;
    let choice = completion
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("the completion has no message"))?;
    let content = choice.message.content.unwrap_or_default();
    if content.trim().is_empty() && choice.finish_reason.as_deref() == Some("length") {
        bail!(
            "the model used its {MAX_ANSWER_TOKENS} tokens up before answering \
             (a reasoning model thinking at length): try another model"
        );
    }
    Ok(content)
}

/// Names in a provider's listing of models that are not text generation
/// (speech, images, moderation), named like chat models often enough to
/// need saying; brainy leaves out the same ones.
const NOT_TEXT: [&str; 11] = [
    "whisper",
    "tts",
    "transcribe",
    "audio",
    "realtime",
    "image",
    "dall-e",
    "sora",
    "moderation",
    "guard",
    "orpheus",
];

/// The text models `options.url` offers (its `/models`, which Ollama,
/// vLLM and the hosted providers all answer), sorted by name; `model`
/// and `timeout` are not used.
pub fn models(options: &Options) -> Result<Vec<String>> {
    let url = format!("{}/models", options.url.trim_end_matches('/'));
    let mut request = agent(options).get(&url);
    if let Some(key) = &options.key {
        request = request.header("Authorization", &format!("Bearer {key}"));
    }
    let mut response = request
        .call()
        .with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    let text = response.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!("{url} answered {status}: {}", text.trim());
    }
    let listing: Listing = serde_json::from_str(&text).context("not a list of models")?;
    let mut names: Vec<String> = listing
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| {
            let id = id.to_lowercase();
            !NOT_TEXT.iter().any(|n| id.contains(n))
        })
        .collect();
    names.sort();
    Ok(names)
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    id: String,
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use serde_json::json;

    use super::*;

    /// One HTTP exchange with a canned answer; the request it read.
    fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let (url, handle) = serve_each(vec![(status.to_string(), String::new(), body.to_string())]);
        (
            url,
            std::thread::spawn(move || handle.join().unwrap().remove(0)),
        )
    }

    /// One exchange per answer (status, extra headers, body), in turn;
    /// the requests read.
    fn serve_each(
        answers: Vec<(String, String, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            answers
                .into_iter()
                .map(|(status, headers, body)| {
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    exchange(&listener, &response)
                })
                .collect()
        });
        (url, handle)
    }

    fn exchange(listener: &TcpListener, response: &str) -> String {
        {
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
        }
    }

    #[test]
    fn a_request_over_the_rate_waits_and_goes_again() {
        let completion = json!({"choices": [{"message": {"content": "Done."}}]}).to_string();
        let limited = r#"{"error":{"code":"rate_limit_exceeded"}}"#.to_string();
        let (url, server) = serve_each(vec![
            (
                "429 Too Many Requests".into(),
                "retry-after: 0\r\n".into(),
                limited.clone(),
            ),
            (
                "413 Payload Too Large".into(),
                "retry-after: 0\r\n".into(),
                limited,
            ),
            ("200 OK".into(), String::new(), completion),
        ]);
        let options = options(url);
        let answer = complete(&agent(&options), &options, json!({"model": "test"})).unwrap();
        assert_eq!(answer, "Done.");
        assert_eq!(server.join().unwrap().len(), 3);
        // Other refusals are errors at once.
        let (url, server) = serve("400 Bad Request", r#"{"error":"no"}"#);
        let options = super::tests::options(url);
        assert!(complete(&agent(&options), &options, json!({"model": "test"})).is_err());
        server.join().unwrap();
    }

    fn options(url: String) -> Options {
        Options {
            url,
            model: "test".into(),
            key: Some("secret".into()),
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn a_completion_is_asked_of_the_endpoint() {
        let completion = json!({"choices": [{"message": {"content": "Done."}}]}).to_string();
        let (url, server) = serve("200 OK", &completion);
        let options = options(url);
        let answer = complete(&agent(&options), &options, json!({"model": "test"})).unwrap();
        assert_eq!(answer, "Done.");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions"));
        assert!(request.contains("Bearer secret"));
        assert!(
            request.contains(r#""max_completion_tokens":8192"#)
                || request.contains(r#""max_completion_tokens": 8192"#),
            "{request}"
        );
    }

    #[test]
    fn an_answer_cut_before_its_text_says_why() {
        let completion = json!({"choices": [{
            "message": {"content": "", "reasoning": "hm"},
            "finish_reason": "length",
        }]})
        .to_string();
        let (url, server) = serve("200 OK", &completion);
        let options = options(url);
        let error = complete(&agent(&options), &options, json!({})).unwrap_err();
        server.join().unwrap();
        assert!(format!("{error:#}").contains("tokens up"), "{error:#}");
    }

    #[test]
    fn the_text_models_are_listed() {
        let listing = json!({"data": [
            {"id": "qwen/qwen3.8-27b"},
            {"id": "whisper-large-v3"},
            {"id": "llama-3.3-70b"},
            {"id": "meta-llama/llama-guard-4-12b"},
        ]})
        .to_string();
        let (url, server) = serve("200 OK", &listing);
        let options = options(url);
        assert_eq!(
            models(&options).unwrap(),
            ["llama-3.3-70b", "qwen/qwen3.8-27b"]
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /v1/models"));
        assert!(request.contains("Bearer secret"));
    }

    #[test]
    fn a_failed_request_says_the_status() {
        let (url, server) = serve("500 Internal Server Error", r#"{"error":"no model"}"#);
        let options = options(url);
        let error = complete(&agent(&options), &options, json!({})).unwrap_err();
        server.join().unwrap();
        assert!(format!("{error:#}").contains("500"), "{error:#}");
    }
}
