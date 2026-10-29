//! The meeting as a page in brainy, through its REST API (`/api/v1`, a
//! personal token: brainy's apps/server/docs/mcp.md, "REST API"). The page
//! is written by the person's own token, so it lands only where they can
//! write; logbook-server never holds brainy credentials.

use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::finishing::HttpStatus;
use crate::shots::{self, Piece};
use crate::transcript::escape;

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
    /// The block's own Markdown, without nested blocks.
    #[serde(default)]
    markdown: String,
    /// 0 for the page's own blocks.
    #[serde(default)]
    depth: u32,
}

#[derive(Deserialize)]
struct Inserted {
    block_ids: Vec<String>,
}

#[derive(Deserialize)]
struct Uploaded {
    block_id: String,
}

impl Brainy {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        Ok(Self {
            base: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: crate::remote::http_builder().build()?,
        })
    }

    /// Whether brainy answers and the token opens it: its workspaces read.
    pub async fn check(&self) -> Result<()> {
        let _: serde_json::Value = self
            .send(Method::GET, "/api/v1/workspaces", &serde_json::Value::Null)
            .await?;
        Ok(())
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

    async fn blocks(&self, page: &str) -> Result<Vec<Block>> {
        let blocks: Blocks = self
            .send(
                Method::GET,
                &format!("/api/v1/pages/{page}/blocks"),
                &serde_json::Value::Null,
            )
            .await?;
        Ok(blocks.blocks)
    }

    /// The id of the page's first block: the header.
    pub async fn first_block(&self, page: &str) -> Result<String> {
        let blocks = self.blocks(page).await?;
        let first = blocks
            .into_iter()
            .next()
            .context("the page has no blocks")?;
        Ok(first.block_id)
    }

    /// Rewrites the page's first block (one block of Markdown): the
    /// header, written once the meeting is over.
    pub async fn rewrite_first_block(&self, page: &str, markdown: &str) -> Result<()> {
        let first = self.first_block(page).await?;
        self.update_block(&first, markdown).await
    }

    async fn update_block(&self, block: &str, markdown: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                Method::PATCH,
                &format!("/api/v1/blocks/{block}"),
                &json!({ "content": markdown }),
            )
            .await?;
        Ok(())
    }

    /// Inserts Markdown right after a block; the new top-level blocks' ids.
    pub async fn insert_after(&self, block: &str, markdown: &str) -> Result<Vec<String>> {
        let mut after = block.to_string();
        let mut ids = Vec::new();
        for part in split(markdown, PART) {
            if part.trim().is_empty() {
                continue;
            }
            let inserted: Inserted = self
                .send(
                    Method::POST,
                    "/api/v1/blocks",
                    &json!({ "content": part, "after_block_id": after }),
                )
                .await?;
            if let Some(last) = inserted.block_ids.last() {
                after = last.clone();
            }
            ids.extend(inserted.block_ids);
        }
        Ok(ids)
    }

    /// `insert_after`, with the screenshots the Markdown shows (shots.rs)
    /// uploaded from `images` as image blocks in their places. One whose
    /// file is gone is left out with a warning.
    pub async fn insert_with_images(
        &self,
        block: &str,
        markdown: &str,
        images: &Path,
    ) -> Result<Vec<String>> {
        let mut after = block.to_string();
        let mut ids = Vec::new();
        for piece in shots::pieces(markdown) {
            let added = match piece {
                Piece::Text(text) => self.insert_after(&after, &text).await?,
                Piece::Image { name, caption } => {
                    let path = images.join(&name);
                    let Ok(data) = std::fs::read(&path) else {
                        tracing::warn!("the screenshot {} is gone", path.display());
                        continue;
                    };
                    vec![self.upload_image(&after, &data, &name, &caption).await?]
                }
            };
            if let Some(last) = added.last() {
                after = last.clone();
            }
            ids.extend(added);
        }
        Ok(ids)
    }

    /// Uploads a PNG as an image block right after a block; its id.
    pub async fn upload_image(
        &self,
        after: &str,
        png: &[u8],
        name: &str,
        caption: &str,
    ) -> Result<String> {
        let mut body = json!({
            "data": base64::engine::general_purpose::STANDARD.encode(png),
            "name": name,
            "after_block_id": after,
        });
        if !caption.is_empty() {
            body["caption"] = json!(caption);
        }
        let uploaded: Uploaded = self.send(Method::POST, "/api/v1/images", &body).await?;
        Ok(uploaded.block_id)
    }

    /// Deletes blocks (they stay in the page's history); those already
    /// gone are not an error.
    pub async fn delete_blocks(&self, page: &str, ids: &[String]) -> Result<()> {
        let alive: Vec<String> = self
            .blocks(page)
            .await?
            .into_iter()
            .map(|b| b.block_id)
            .filter(|id| ids.contains(id))
            .collect();
        for chunk in alive.chunks(200) {
            let _: serde_json::Value = self
                .send(
                    Method::POST,
                    "/api/v1/blocks/delete",
                    &json!({ "block_ids": chunk }),
                )
                .await?;
        }
        Ok(())
    }

    /// Gives the speaker labelled `from` the label `to` on the meeting's
    /// page: the heading line of each of its turns ("**from** · 00:04"),
    /// and its name in the header's list of speakers. Returns how many
    /// blocks changed.
    pub async fn rename_speaker(&self, page: &str, from: &str, to: &str) -> Result<usize> {
        let blocks = self.blocks(page).await?;
        let mut changed = 0;
        for (i, block) in blocks.iter().enumerate() {
            let renamed = if i == 0 {
                renamed_header(&block.markdown, from, to)
            } else if block.depth == 0 {
                renamed_turn(&block.markdown, from, to)
            } else {
                None
            };
            if let Some(markdown) = renamed {
                self.update_block(&block.block_id, &markdown).await?;
                changed += 1;
            }
        }
        Ok(changed)
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
            return Err(HttpStatus {
                status: status.as_u16(),
                message: format!(
                    "brainy answered {status} to {method} {path}: {}",
                    text.trim()
                ),
            }
            .into());
        }
        Ok(response.json().await?)
    }
}

/// A turn's heading line with the speaker's new label, if it is `from`'s.
fn renamed_turn(markdown: &str, from: &str, to: &str) -> Option<String> {
    let prefix = format!("**{}** · ", escape(from));
    let rest = markdown.strip_prefix(&prefix)?;
    Some(format!("**{}** · {rest}", escape(to)))
}

/// The header ("Recorded … · 12 min · Marco, Speaker 2") with `from`
/// renamed in its list of speakers, the part after the last " · " of its
/// first line.
fn renamed_header(markdown: &str, from: &str, to: &str) -> Option<String> {
    let (first, rest) = markdown.split_once('\n').unwrap_or((markdown, ""));
    let (head, list) = first.rsplit_once(" · ")?;
    let from = escape(from);
    let mut found = false;
    let names: Vec<String> = list
        .split(", ")
        .map(|n| {
            if n == from {
                found = true;
                escape(to)
            } else {
                n.to_string()
            }
        })
        .collect();
    if !found {
        return None;
    }
    let mut out = format!("{head} · {}", names.join(", "));
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(rest);
    }
    Some(out)
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
    fn a_speaker_is_renamed_in_its_turns_and_the_header() {
        assert_eq!(
            renamed_turn("**Speaker 2** · 00:04", "Speaker 2", "Luca").as_deref(),
            Some("**Luca** · 00:04")
        );
        assert_eq!(
            renamed_turn("**Speaker 22** · 00:04", "Speaker 2", "Luca"),
            None
        );
        assert_eq!(renamed_turn("Speaker 2 said so", "Speaker 2", "Luca"), None);
        assert_eq!(
            renamed_header(
                "Recorded 2026-10-27 09:15 · 12 min · Marco, Speaker 2, Speaker 3 (Luca?)",
                "Speaker 3 (Luca?)",
                "Luca"
            )
            .as_deref(),
            Some("Recorded 2026-10-27 09:15 · 12 min · Marco, Speaker 2, Luca")
        );
        assert_eq!(
            renamed_header("Recorded … · 1 min · Marco", "Anna", "Bo"),
            None
        );
    }

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
