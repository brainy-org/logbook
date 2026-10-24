//! The meeting as a page in brainy, through its REST API (`/api/v1`, a
//! personal token: brainy's apps/server/docs/mcp.md, "REST API"). The page
//! is written by the person's own token, so it lands only where they can
//! write; logbook-server never holds brainy credentials.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

/// brainy refuses Markdown longer than 200 000 characters in one call
/// (`markdown` in its operations catalog); a long meeting is sent in parts.
const PART: usize = 150_000;

pub struct Brainy {
    base: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Created {
    page_id: String,
    url: String,
}

impl Brainy {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        // reqwest is built without a crypto provider (rustls on ring, as
        // galley-client).
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            base: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::Client::builder().build()?,
        })
    }

    /// Creates the page under `parent` (a page id), or at the top of the
    /// token owner's default workspace; returns its address.
    pub async fn create_page(
        &self,
        title: &str,
        parent: Option<&str>,
        markdown: &str,
    ) -> Result<String> {
        let mut parts = split(markdown, PART).into_iter();
        let mut body =
            json!({ "title": title, "icon": "🎙️", "content": parts.next().unwrap_or("") });
        if let Some(parent) = parent {
            body["parent_id"] = json!(parent);
        }
        let created: Created = self.post("/api/v1/pages", &body).await?;
        for part in parts {
            let path = format!("/api/v1/pages/{}/append", created.page_id);
            let _: serde_json::Value = self.post(&path, &json!({ "content": part })).await?;
        }
        Ok(created.url)
    }

    async fn post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}{path}", self.base);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("could not reach {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!("brainy answered {status} to POST {path}: {}", text.trim());
        }
        Ok(response.json().await?)
    }
}

/// `text` in parts of at most `max` bytes, cut between paragraphs.
fn split(text: &str, max: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while rest.len() > max {
        // The longest prefix of at most `max` bytes that ends on a character.
        let limit = (0..=max)
            .rev()
            .find(|&i| rest.is_char_boundary(i))
            .unwrap_or(0);
        // A paragraph that fits may end with its "\n\n" just past the limit.
        let window = rest
            .get(..limit + 2)
            .or_else(|| rest.get(..limit + 1))
            .unwrap_or(&rest[..limit]);
        let cut = match window.rfind("\n\n") {
            Some(i) if i > 0 && i <= limit => i,
            _ => limit,
        };
        parts.push(&rest[..cut]);
        rest = rest[cut..].trim_start_matches('\n');
    }
    parts.push(rest);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_between_paragraphs() {
        let text = "aaaa\n\nbbbb\n\ncccc";
        assert_eq!(split(text, 11), vec!["aaaa\n\nbbbb", "cccc"]);
        assert_eq!(split("short", 100), vec!["short"]);
        // No paragraph to cut at, and a character across the limit.
        assert_eq!(split("perché", 6), vec!["perch", "é"]);
    }
}
