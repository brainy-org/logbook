//! The meeting's report, written by a language model from the final
//! transcript and the notes the person took during the meeting, as
//! Granola does: a summary, the notes completed from what was said, the
//! decisions and the action items (docs/pipeline.md, "Report").
//!
//! One request to the same OpenAI-compatible endpoint as the clean-up
//! (polish.rs), with the whole transcript: a meeting of a few hours fits
//! the context of the models it is used with. The transcript comes with
//! the speakers' names the client gave them (the server never sees the
//! client's voices), so the report names people as the page does.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::polish::{self, Options};
use crate::protocol::ReportRequest;

/// The longest transcript sent, in characters: some 90 000 tokens, six
/// to nine hours of speech (a 28-minute meeting was 16 800 characters). A longer one is refused rather than cut:
/// a report of part of the meeting reads as a report of all of it.
pub const MAX_TRANSCRIPT: usize = 320_000;
/// The longest notes sent.
pub const MAX_NOTES: usize = 40_000;

const INSTRUCTIONS: &str = "\
You write the report of a meeting from its transcript and from the notes the person \
recording it took during the meeting, as a careful assistant would for the minutes.

First decide which language most of the transcript is in. Write the whole report, headings \
included, in that language, whatever the language of these instructions or of the notes. \
Answer with Markdown only, in this order, leaving out a section with nothing to put in it. \
Each heading is the word in angle brackets in the transcript's language: an English meeting \
has \"## Summary\", \"## Notes\", \"## Decisions\", \"## Action items\"; an Italian one \
\"## Riepilogo\", \"## Note\", \"## Decisioni\", \"## Azioni\"; another language its own words.

## <Summary>
Three to six sentences: what the meeting was about and how it ended.

## <Notes>
Only when there are notes: every point of the person's notes, in their order and in their \
words, each completed with what the transcript says about it as nested bullets. \
The notes say what mattered to the person: follow them.

## <Decisions>
A bulleted list of what was decided.

## <Action items>
A task list (- [ ] ...), one per line, each starting with a verb, with who owns it \
(\" — Name\") and when it is due when the meeting says so.

Rules:
- Only what the transcript or the notes say: never invent names, numbers, dates or decisions.
- Call people as the transcript does. \"Speaker 2\" is someone whose name is not known: \
keep the label, unless the meeting itself makes their name certain.
- The transcript comes from speech recognition: read misheard words for what they must have been.
- No preamble, no closing remark, no title above the first heading.";

/// The report's Markdown, from the model at `options`. Blocks for one
/// request: run it off the async threads.
pub fn write(agent: &polish::Agent, options: &Options, request: &ReportRequest) -> Result<String> {
    if request.transcript.trim().is_empty() {
        bail!("the meeting has no transcript to report on");
    }
    if request.transcript.chars().count() > MAX_TRANSCRIPT {
        bail!(
            "the meeting is too long for one report (its transcript is over \
             {MAX_TRANSCRIPT} characters): the transcript is complete, only the report is not written"
        );
    }
    if request.notes.chars().count() > MAX_NOTES {
        bail!("the notes are longer than {MAX_NOTES} characters");
    }
    let answer = polish::complete(agent, options, body(&options.model, request))?;
    let markdown = clean(&answer);
    if markdown.is_empty() {
        bail!("the model wrote an empty report");
    }
    Ok(markdown)
}

fn body(model: &str, request: &ReportRequest) -> Value {
    let mut user = String::new();
    if !request.title.trim().is_empty() {
        user.push_str(&format!("Meeting: {}\n\n", request.title.trim()));
    }
    if request.notes.trim().is_empty() {
        user.push_str("Notes: none.\n\n");
    } else {
        user.push_str(&format!(
            "Notes:\n<notes>\n{}\n</notes>\n\n",
            request.notes.trim()
        ));
    }
    user.push_str(&format!(
        "Transcript:\n<transcript>\n{}\n</transcript>",
        request.transcript.trim()
    ));
    json!({
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": INSTRUCTIONS},
            {"role": "user", "content": user},
        ],
    })
}

/// The Markdown past what models put around it: a `<think>` block, a
/// code fence around the whole answer.
fn clean(answer: &str) -> String {
    let answer = match answer.rfind("</think>") {
        Some(i) => &answer[i + "</think>".len()..],
        None => answer,
    };
    let answer = answer.trim();
    let unfenced = answer
        .strip_prefix("```markdown")
        .or_else(|| answer.strip_prefix("```md"))
        .or_else(|| answer.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"));
    unfenced.unwrap_or(answer).trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use super::*;

    #[test]
    fn what_models_put_around_the_report_is_dropped() {
        assert_eq!(
            clean("<think>hm</think>\n## Summary\nOk."),
            "## Summary\nOk."
        );
        assert_eq!(
            clean("```markdown\n## Summary\nOk.\n```"),
            "## Summary\nOk."
        );
        assert_eq!(clean("  ## Summary\nOk.  "), "## Summary\nOk.");
    }

    #[test]
    fn notes_and_transcript_go_to_the_model() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            // Headers, then the body by Content-Length.
            loop {
                let n = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
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
            let answer =
                json!({"choices": [{"message": {"content": "## Summary\nDone."}}]}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{answer}",
                answer.len()
            )
            .unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        let options = Options {
            url: format!("http://{address}/v1"),
            model: "m".into(),
            key: None,
            timeout: Duration::from_secs(5),
        };
        let request = ReportRequest {
            title: "Weekly".into(),
            notes: "- budget".into(),
            transcript: "**Luca** · 00:04\n\nIl budget è approvato.".into(),
        };
        let report = write(&polish::agent(&options), &options, &request).unwrap();
        assert_eq!(report, "## Summary\nDone.");
        let sent = server.join().unwrap();
        assert!(sent.contains("- budget"));
        assert!(sent.contains("Il budget"));
        let empty = ReportRequest {
            transcript: " ".into(),
            ..request
        };
        assert!(write(&polish::agent(&options), &options, &empty).is_err());
    }
}
