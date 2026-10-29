//! Who may use the server: a token per person, made and revoked on the
//! admin page (`/`) with the admin token, and sent by the client as
//! `Authorization: Bearer <token>` on every request (docs/protocol.md,
//! "Tokens"). The server keeps only their SHA-256: a copy of its files
//! lets no one in.
//!
//! In `tokens/` in its data folder, which replicas of the server may share
//! (engine store.rs): `<hash>.json` per person, made once and deleted when
//! revoked, and `<hash>.used`, its last use, written by whichever server
//! it was used on. The id of a token is its hash. Two admins making tokens
//! at once make two files; a use noted as the token is revoked writes a
//! `.used` beside no token, which lets no one in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use logbook_engine::store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// What a person's token starts with: told apart from other secrets at a
/// glance (in a leak scanner, in a settings field).
const PREFIX: &str = "lbk_";
/// How often a token's last use is written: not at every request (a
/// meeting polls every few seconds).
const LAST_USED_EVERY: u64 = 60;
/// The longest name of a token's owner.
const MAX_NAME: usize = 100;

/// Who made a request: a token's owner, or no one with `--no-auth`.
#[derive(Clone, Debug)]
pub struct Caller(pub String);

pub struct Auth {
    /// False with `--no-auth`: every request is let in.
    required: bool,
    admin: [u8; 32],
    /// Where the admin token is, when it was not given.
    pub admin_file: Option<PathBuf>,
    /// `tokens/` in the data folder.
    dir: PathBuf,
    /// When this server last wrote each token's use, by hash.
    noted: Mutex<HashMap<String, u64>>,
}

/// A person's token, as kept: its hash is the file's name.
#[derive(Serialize, Deserialize)]
struct Kept {
    name: String,
    /// Seconds since the Unix epoch.
    created: u64,
}

/// A token as the admin page shows it.
#[derive(Serialize)]
struct UserView {
    id: String,
    name: String,
    created: u64,
    last_used: Option<u64>,
}

/// A token just made: the only time it is shown.
#[derive(Serialize)]
struct Created {
    id: String,
    name: String,
    token: String,
}

#[derive(Deserialize)]
pub struct NewUser {
    name: String,
}

impl Auth {
    /// The tokens in `data`, and the admin token: `admin` if given, else
    /// the one in `data/admin.token`, made at first start.
    pub fn open(data: &Path, admin: Option<String>, required: bool) -> Result<Self> {
        let dir = data.join("tokens");
        std::fs::create_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
        let (admin, admin_file) = match admin
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
        {
            Some(admin) => (admin, None),
            None => {
                let file = data.join("admin.token");
                (admin_token(&file)?, Some(file))
            }
        };
        let moved = migrate(data, &dir)?;
        if moved > 0 {
            info!(moved, "tokens.json moved to tokens/");
        }
        Ok(Self {
            required,
            admin: digest(&admin),
            admin_file,
            dir,
            noted: Mutex::default(),
        })
    }

    fn path(&self, hash: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{hash}.{extension}"))
    }

    pub fn count(&self) -> Result<usize> {
        Ok(self.hashes()?.len())
    }

    /// The owner of `token`, its last use noted; None if no one owns it.
    fn check(&self, token: &str) -> Result<Option<String>> {
        let hash = hex(&digest(token));
        let Some(bytes) = store::read(&self.path(&hash, "json"))? else {
            return Ok(None);
        };
        let kept: Kept = serde_json::from_slice(&bytes).context("a token's file")?;
        let now = now();
        let due = {
            let mut noted = self.noted.lock().unwrap();
            let due = noted.get(&hash).is_none_or(|t| now >= t + LAST_USED_EVERY);
            if due {
                noted.insert(hash.clone(), now);
            }
            due
        };
        if due
            && let Err(error) =
                store::replace(&self.path(&hash, "used"), now.to_string().as_bytes())
        {
            warn!("could not note a token's use: {error:#}");
        }
        Ok(Some(kept.name))
    }

    fn create(&self, name: &str) -> Result<Created> {
        let name = valid_name(name)?;
        let token = format!("{PREFIX}{}", store::random_hex(32)?);
        let hash = hex(&digest(&token));
        let kept = Kept {
            name: name.to_string(),
            created: now(),
        };
        if !store::create(
            &self.path(&hash, "json"),
            &serde_json::to_vec_pretty(&kept)?,
        )? {
            bail!("a token made twice");
        }
        // Shown only once: it must still be there after a crash.
        store::sync_dir(&self.dir)?;
        Ok(Created {
            id: hash,
            name: kept.name,
            token,
        })
    }

    /// False if there is no token `id`.
    fn revoke(&self, id: &str) -> Result<bool> {
        if !is_hash(id) {
            return Ok(false);
        }
        // The token first: once it is gone, no server lets it in.
        if !store::remove(&self.path(id, "json"))? {
            return Ok(false);
        }
        store::remove(&self.path(id, "used"))?;
        store::sync_dir(&self.dir)?;
        self.noted.lock().unwrap().remove(id);
        Ok(true)
    }

    fn list(&self) -> Result<Vec<UserView>> {
        let mut users = Vec::new();
        for hash in self.hashes()? {
            // Revoked since the folder was read.
            let Some(bytes) = store::read(&self.path(&hash, "json"))? else {
                continue;
            };
            let kept: Kept = serde_json::from_slice(&bytes).context("a token's file")?;
            let last_used = store::read(&self.path(&hash, "used"))?
                .and_then(|b| String::from_utf8_lossy(&b).trim().parse().ok());
            users.push(UserView {
                id: hash,
                name: kept.name,
                created: kept.created,
                last_used,
            });
        }
        users.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        Ok(users)
    }

    /// The tokens' hashes, from the names in the folder.
    fn hashes(&self) -> Result<Vec<String>> {
        let entries = std::fs::read_dir(&self.dir)
            .with_context(|| format!("could not read {}", self.dir.display()))?;
        let mut hashes = Vec::new();
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(hash) = name.strip_suffix(".json").filter(|h| is_hash(h)) {
                hashes.push(hash.to_string());
            }
        }
        Ok(hashes)
    }
}

/// The admin token in `file`, made if there is none. Two servers starting
/// at once on one data folder make one: the other reads it.
fn admin_token(file: &Path) -> Result<String> {
    let read = || -> Result<Option<String>> {
        Ok(store::read(file)?
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .filter(|a| !a.is_empty()))
    };
    if let Some(admin) = read()? {
        return Ok(admin);
    }
    let admin = format!("{PREFIX}admin_{}", store::random_hex(32)?);
    if store::create(file, admin.as_bytes())? {
        info!("made the admin token");
        return Ok(admin);
    }
    match read()? {
        Some(made) => Ok(made),
        // Empty: written by a server before this one, which stopped
        // halfway.
        None => {
            store::replace(file, admin.as_bytes())?;
            info!("made the admin token");
            Ok(admin)
        }
    }
}

/// `tokens.json`, where a server before replicas kept every token, moved
/// to `tokens/`, a file each. Renamed first, so that the server that
/// moves it is the one whose rename succeeded; one that finds it renamed
/// (that server stopped halfway) moves it again, and what is moved
/// already stays.
fn migrate(data: &Path, dir: &Path) -> Result<usize> {
    #[derive(Deserialize)]
    struct Old {
        name: String,
        hash: String,
        created: u64,
        last_used: Option<u64>,
    }
    let old = data.join("tokens.json");
    let moving = data.join("tokens.json.moving");
    match std::fs::rename(&old, &moving) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("could not move {}", old.display()));
        }
    }
    let Some(bytes) = store::read(&moving)? else {
        return Ok(0);
    };
    let users: Vec<Old> = serde_json::from_slice(&bytes)
        .with_context(|| format!("could not read {}", moving.display()))?;
    for user in &users {
        if !is_hash(&user.hash) {
            bail!("{}: a hash that is not one", moving.display());
        }
        let kept = Kept {
            name: user.name.clone(),
            created: user.created,
        };
        store::create(
            &dir.join(format!("{}.json", user.hash)),
            &serde_json::to_vec_pretty(&kept)?,
        )?;
        if let Some(used) = user.last_used {
            store::replace(
                &dir.join(format!("{}.used", user.hash)),
                used.to_string().as_bytes(),
            )?;
        }
    }
    store::sync_dir(dir)?;
    store::remove(&moving)?;
    Ok(users.len())
}

fn valid_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        bail!("a name of 1 to {MAX_NAME} characters");
    }
    Ok(name)
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// The routes of meetings: a person's token, or 401. The caller goes in
/// the request's extensions, for the logs.
pub async fn require_token(
    State(auth): State<Arc<Auth>>,
    mut request: Request,
    next: Next,
) -> Response {
    let caller = if auth.required {
        let Some(token) = bearer(request.headers()).map(str::to_string) else {
            return (StatusCode::UNAUTHORIZED, "a valid token is needed").into_response();
        };
        match blocking(&auth, move |auth| auth.check(&token)).await {
            Ok(Some(name)) => name,
            Ok(None) => {
                return (StatusCode::UNAUTHORIZED, "a valid token is needed").into_response();
            }
            // Not a 401: the person would be told their token is refused.
            Err(error) => {
                warn!("could not check a token: {error:#}");
                return (StatusCode::SERVICE_UNAVAILABLE, "could not check the token")
                    .into_response();
            }
        }
    } else {
        "anyone".to_string()
    };
    request.extensions_mut().insert(Caller(caller));
    next.run(request).await
}

/// `f` off the async threads: the tokens are files, on a volume that may
/// be a network's.
async fn blocking<T: Send + 'static>(
    auth: &Arc<Auth>,
    f: impl FnOnce(&Auth) -> Result<T> + Send + 'static,
) -> Result<T> {
    let auth = auth.clone();
    tokio::task::spawn_blocking(move || f(&auth)).await?
}

/// The admin page's API: the admin token, or 401. Compared by hash: the
/// time taken tells nothing of the token.
pub async fn require_admin(
    State(auth): State<Arc<Auth>>,
    request: Request,
    next: Next,
) -> Response {
    match bearer(request.headers()) {
        Some(token) if digest(token) == auth.admin => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, "the admin token is needed").into_response(),
    }
}

pub async fn list(State(auth): State<Arc<Auth>>) -> Response {
    match blocking(&auth, |auth| auth.list()).await {
        Ok(users) => Json(users).into_response(),
        Err(error) => failed("could not list the tokens", error),
    }
}

pub async fn create(State(auth): State<Arc<Auth>>, Json(new): Json<NewUser>) -> Response {
    let name = match valid_name(&new.name) {
        Ok(name) => name.to_string(),
        Err(error) => return (StatusCode::BAD_REQUEST, format!("{error:#}")).into_response(),
    };
    match blocking(&auth, move |auth| auth.create(&name)).await {
        Ok(created) => {
            info!(name = created.name, "token made");
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(error) => failed("could not make a token", error),
    }
}

pub async fn revoke(State(auth): State<Arc<Auth>>, UrlPath(id): UrlPath<String>) -> Response {
    let revoked = id.clone();
    match blocking(&auth, move |auth| auth.revoke(&revoked)).await {
        Ok(true) => {
            info!(id, "token revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => failed("could not revoke a token", error),
    }
}

fn failed(what: &str, error: anyhow::Error) -> Response {
    warn!("{what}: {error:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("{what}: {error:#}"),
    )
        .into_response()
}

/// The admin page, at `/`: it asks for the admin token and calls the API
/// above. Nothing from elsewhere, no framing: the token is typed into it.
/// Brainy's look (its tokens, as the desktop app's `index.css` has them).
pub async fn page() -> Response {
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 font-src 'self'; img-src data:; connect-src 'self'; frame-ancestors 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(include_str!("admin.html")),
    )
        .into_response()
}

/// Brainy's typeface, Geist (SIL OFL 1.1, `assets/geist-OFL.txt`), Latin
/// only: the page asks nothing of another site.
pub async fn font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("assets/geist.woff2").as_slice(),
    )
        .into_response()
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("logbook-auth-{}", store::random_hex(8).unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn check(auth: &Auth, token: &str) -> Option<String> {
        auth.check(token).unwrap()
    }

    #[test]
    fn a_token_lets_in_until_revoked_and_survives_a_restart() {
        let dir = dir();
        let auth = Auth::open(&dir, Some("admin".into()), true).unwrap();
        let made = auth.create(" Luca ").unwrap();
        assert!(made.token.starts_with(PREFIX));
        assert_eq!(check(&auth, &made.token).as_deref(), Some("Luca"));
        assert_eq!(check(&auth, "lbk_wrong"), None);

        let again = Auth::open(&dir, Some("admin".into()), true).unwrap();
        assert_eq!(check(&again, &made.token).as_deref(), Some("Luca"));
        for entry in std::fs::read_dir(dir.join("tokens")).unwrap() {
            let kept = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!kept.contains(&made.token), "the token itself is kept");
        }
        let listed = again.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].last_used.is_some());

        assert!(again.revoke(&made.id).unwrap());
        assert_eq!(check(&again, &made.token), None);
        assert!(!again.revoke(&made.id).unwrap());
        assert!(!again.revoke("../admin.token").unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn servers_on_one_folder_make_tokens_at_once_and_see_a_revocation_at_once() {
        let dir = dir();
        let servers: Vec<Arc<Auth>> = (0..4)
            .map(|_| Arc::new(Auth::open(&dir, Some("admin".into()), true).unwrap()))
            .collect();
        let made: Vec<Created> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..32)
                .map(|i| {
                    let auth = servers[i % servers.len()].clone();
                    scope.spawn(move || auth.create(&format!("person {i}")).unwrap())
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        for auth in &servers {
            assert_eq!(auth.list().unwrap().len(), made.len());
            for created in &made {
                assert_eq!(check(auth, &created.token), Some(created.name.clone()));
            }
        }

        // One server revokes; on every other one the token is refused
        // from then on, and the use they note does not bring it back.
        let gone = &made[0];
        assert!(servers[0].revoke(&gone.id).unwrap());
        for auth in &servers {
            auth.noted.lock().unwrap().clear();
            assert_eq!(check(auth, &gone.token), None);
        }
        store::replace(&servers[1].path(&gone.id, "used"), b"1").unwrap();
        assert_eq!(check(&servers[2], &gone.token), None);
        assert_eq!(servers[3].list().unwrap().len(), made.len() - 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tokens_json_is_moved_to_a_file_each() {
        let dir = dir();
        let token = "lbk_old";
        let hash = hex(&digest(token));
        let old =
            format!(r#"[{{"id":"ab","name":"Luca","hash":"{hash}","created":5,"last_used":7}}]"#);
        std::fs::write(dir.join("tokens.json"), old).unwrap();
        let auth = Auth::open(&dir, Some("admin".into()), true).unwrap();
        assert!(!dir.join("tokens.json").exists());
        assert!(!dir.join("tokens.json.moving").exists());
        assert_eq!(check(&auth, token).as_deref(), Some("Luca"));
        let listed = auth.list().unwrap();
        assert_eq!(
            (listed[0].created, listed[0].id.as_str()),
            (5, hash.as_str())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_admin_token_is_made_once_by_servers_starting_together() {
        let dir = dir();
        let made: Vec<String> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let dir = &dir;
                    scope.spawn(move || {
                        Auth::open(dir, None, true).unwrap();
                        std::fs::read_to_string(dir.join("admin.token")).unwrap()
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(made.iter().all(|m| m == &made[0]));
        Auth::open(&dir, None, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("admin.token")).unwrap(),
            made[0]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("admin.token"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_name_is_needed() {
        let dir = dir();
        let auth = Auth::open(&dir, Some("admin".into()), true).unwrap();
        assert!(auth.create("  ").is_err());
        assert!(auth.create(&"x".repeat(MAX_NAME + 1)).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
