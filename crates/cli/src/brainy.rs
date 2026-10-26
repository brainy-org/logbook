//! The meeting as a page in brainy, through its REST API (`/api/v1`, a
//! personal token: brainy's apps/server/docs/mcp.md, "REST API"). The page
//! is written by the person's own token, so it lands only where they can
//! write; logbook-server never holds brainy credentials.

use anyhow::{Context, Result, bail};
use reqwest::Method;
use serde::{Deserialize, Serialize};
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

/// A page being written: its id, and its address to show.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Page {
    pub id: String,
    pub url: String,
}

/// A page as `list_pages` lists it.
#[derive(Debug, Deserialize)]
struct Listed {
    page_id: String,
    title: String,
    url: String,
}

#[derive(Deserialize)]
struct Blocks {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    block_id: String,
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
    /// token owner's default workspace.
    pub async fn create_page(
        &self,
        title: &str,
        parent: Option<&str>,
        markdown: &str,
    ) -> Result<Page> {
        let mut parts = split(markdown, PART).into_iter();
        let mut body =
            json!({ "title": title, "icon": "🎙️", "content": parts.next().unwrap_or("") });
        if let Some(parent) = parent {
            body["parent_id"] = json!(parent);
        }
        let created: Created = self.send(Method::POST, "/api/v1/pages", &body).await?;
        let page = Page {
            id: created.page_id,
            url: created.url,
        };
        for part in parts {
            self.append(&page.id, part).await?;
        }
        Ok(page)
    }

    /// The id of the page `reference` names: its id, its address (as the
    /// browser shows it, `…/w/<workspace>/<page>`), or its path of titles
    /// from the top of the default workspace, "Engineering/Meetings" (any
    /// case). A title that matches no page or more than one is an error
    /// that lists what is there, so a meeting never lands somewhere else.
    pub async fn resolve_page(&self, reference: &str) -> Result<String> {
        let reference = reference.trim();
        if is_id(reference) {
            return Ok(reference.to_string());
        }
        if reference.starts_with("http://") || reference.starts_with("https://") {
            return id_in_url(reference)
                .with_context(|| format!("{reference} is not the address of a brainy page"));
        }
        let mut parent: Option<Listed> = None;
        for name in reference
            .split('/')
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let path = match &parent {
                Some(p) => format!("/api/v1/pages?parent_id={}", p.page_id),
                None => "/api/v1/pages".to_string(),
            };
            let pages: Vec<Listed> = self
                .send(Method::GET, &path, &serde_json::Value::Null)
                .await?;
            let under = parent
                .as_ref()
                .map_or("the top of your workspace".to_string(), |p| {
                    format!("\"{}\"", p.title)
                });
            parent = Some(pick(pages, name).map_err(|e| e.context(format!("under {under}")))?);
        }
        Ok(parent.context("--parent names no page")?.page_id)
    }

    /// Adds Markdown at the end of the page.
    pub async fn append(&self, page: &str, markdown: &str) -> Result<()> {
        let path = format!("/api/v1/pages/{page}/append");
        for part in split(markdown, PART) {
            if part.trim().is_empty() {
                continue;
            }
            let _: serde_json::Value = self
                .send(Method::POST, &path, &json!({ "content": part }))
                .await?;
        }
        Ok(())
    }

    /// Rewrites the page's first block (one block of Markdown): the
    /// header, written once the meeting is over.
    pub async fn rewrite_first_block(&self, page: &str, markdown: &str) -> Result<()> {
        let blocks: Blocks = self
            .send(
                Method::GET,
                &format!("/api/v1/pages/{page}/blocks"),
                &serde_json::Value::Null,
            )
            .await?;
        let first = blocks
            .blocks
            .first()
            .context("the page has no blocks to rewrite")?;
        let _: serde_json::Value = self
            .send(
                Method::PATCH,
                &format!("/api/v1/blocks/{}", first.block_id),
                &json!({ "content": markdown }),
            )
            .await?;
        Ok(())
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        method: Method,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}{path}", self.base);
        let mut request = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(&self.token);
        if !body.is_null() {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("could not reach {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!(
                "brainy answered {status} to {method} {path}: {}",
                text.trim()
            );
        }
        Ok(response.json().await?)
    }
}

/// A page id: a UUID, 8-4-4-4-12 hexadecimal digits.
fn is_id(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The page id at the end of a page's address, `…/w/<workspace>/<page>`.
fn id_in_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    is_id(last).then(|| last.to_string())
}

/// The one page titled `name`, any case.
fn pick(pages: Vec<Listed>, name: &str) -> Result<Listed> {
    let wanted = name.to_lowercase();
    let (mut matches, others): (Vec<Listed>, Vec<Listed>) = pages
        .into_iter()
        .partition(|p| p.title.to_lowercase() == wanted);
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => {
            let titles: Vec<String> = others.iter().map(|p| format!("\"{}\"", p.title)).collect();
            if titles.is_empty() {
                bail!("no page is titled \"{name}\", and none is there")
            }
            bail!(
                "no page is titled \"{name}\"; there are {}",
                titles.join(", ")
            )
        }
        _ => {
            let urls: Vec<&str> = matches.iter().map(|p| p.url.as_str()).collect();
            bail!(
                "{} pages are titled \"{name}\": pass the address of one instead ({})",
                matches.len(),
                urls.join(", ")
            )
        }
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

    const ID: &str = "0b5c6a1e-3f2d-4c8b-9a7e-1d2f3a4b5c6d";

    #[test]
    fn a_page_is_found_by_its_id_or_its_address() {
        assert!(is_id(ID));
        assert!(!is_id("Meetings"));
        assert!(!is_id(&ID.replace('-', "x")));
        let url = format!("https://brainy.example.com/w/{ID}/{ID}");
        assert_eq!(id_in_url(&url).as_deref(), Some(ID));
        assert_eq!(id_in_url(&format!("{url}/?x=1#b")).as_deref(), Some(ID));
        assert_eq!(id_in_url("https://brainy.example.com/w/"), None);
    }

    fn listed(title: &str, n: u8) -> Listed {
        Listed {
            page_id: format!("id{n}"),
            title: title.into(),
            url: format!("https://b/w/x/id{n}"),
        }
    }

    #[test]
    fn a_title_names_exactly_one_page() {
        let pages = vec![listed("Meetings", 1), listed("Notes", 2)];
        assert_eq!(pick(pages, "meetings").unwrap().page_id, "id1");
        let missing = pick(vec![listed("Notes", 2)], "Meetings").unwrap_err();
        assert!(missing.to_string().contains("\"Notes\""));
        let twice = pick(
            vec![listed("Meetings", 1), listed("meetings", 2)],
            "Meetings",
        );
        assert!(twice.unwrap_err().to_string().contains("https://b/w/x/id2"));
    }
}
