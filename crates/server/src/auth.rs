//! Who may use the server: a token per person, made and revoked on the
//! admin page (`/`) with the admin token, and sent by the client as
//! `Authorization: Bearer <token>` on every request (docs/protocol.md,
//! "Tokens"). The server keeps only their SHA-256, in `tokens.json` in its
//! data folder: a copy of the file lets no one in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// What a person's token starts with: told apart from other secrets at a
/// glance (in a leak scanner, in a settings field).
const PREFIX: &str = "lbk_";
/// How often a token's last use is written to the file: not at every
/// request (a meeting polls every few seconds).
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
    path: PathBuf,
    users: Mutex<Vec<User>>,
}

/// A person's token, as kept: its hash, never the token.
#[derive(Clone, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    /// SHA-256 of the token, hex.
    hash: String,
    /// Seconds since the Unix epoch.
    created: u64,
    last_used: Option<u64>,
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
        std::fs::create_dir_all(data).with_context(|| format!("{}", data.display()))?;
        let (admin, admin_file) = match admin
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
        {
            Some(admin) => (admin, None),
            None => {
                let file = data.join("admin.token");
                let admin = match std::fs::read_to_string(&file) {
                    Ok(admin) if !admin.trim().is_empty() => admin.trim().to_string(),
                    _ => {
                        let admin = format!("{PREFIX}admin_{}", random_hex(32)?);
                        write_private(&file, admin.as_bytes())?;
                        info!("made the admin token");
                        admin
                    }
                };
                (admin, Some(file))
            }
        };
        let path = data.join("tokens.json");
        let users = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("could not read {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        Ok(Self {
            required,
            admin: digest(&admin),
            admin_file,
            path,
            users: Mutex::new(users),
        })
    }

    pub fn count(&self) -> usize {
        self.users.lock().unwrap().len()
    }

    /// The owner of `token`, its last use noted.
    fn check(&self, token: &str) -> Option<String> {
        let hash = hex(&digest(token));
        let mut users = self.users.lock().unwrap();
        let user = users.iter_mut().find(|u| u.hash == hash)?;
        let now = now();
        let name = user.name.clone();
        if user.last_used.is_none_or(|t| now >= t + LAST_USED_EVERY) {
            user.last_used = Some(now);
            if let Err(error) = save(&self.path, &users) {
                warn!("could not note a token's use: {error:#}");
            }
        }
        Some(name)
    }

    fn create(&self, name: &str) -> Result<Created> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            bail!("a name of 1 to {MAX_NAME} characters");
        }
        let token = format!("{PREFIX}{}", random_hex(32)?);
        let user = User {
            id: random_hex(8)?,
            name: name.to_string(),
            hash: hex(&digest(&token)),
            created: now(),
            last_used: None,
        };
        let mut users = self.users.lock().unwrap();
        users.push(user.clone());
        if let Err(error) = save(&self.path, &users) {
            users.pop();
            return Err(error);
        }
        Ok(Created {
            id: user.id,
            name: user.name,
            token,
        })
    }

    fn revoke(&self, id: &str) -> Result<bool> {
        let mut users = self.users.lock().unwrap();
        let Some(i) = users.iter().position(|u| u.id == id) else {
            return Ok(false);
        };
        let removed = users.remove(i);
        if let Err(error) = save(&self.path, &users) {
            users.insert(i, removed);
            return Err(error);
        }
        Ok(true)
    }

    fn list(&self) -> Vec<UserView> {
        self.users
            .lock()
            .unwrap()
            .iter()
            .map(|u| UserView {
                id: u.id.clone(),
                name: u.name.clone(),
                created: u.created,
                last_used: u.last_used,
            })
            .collect()
    }
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
        match bearer(request.headers()).and_then(|t| auth.check(t)) {
            Some(name) => name,
            None => {
                return (StatusCode::UNAUTHORIZED, "a valid token is needed").into_response();
            }
        }
    } else {
        "anyone".to_string()
    };
    request.extensions_mut().insert(Caller(caller));
    next.run(request).await
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

pub async fn list(State(auth): State<Arc<Auth>>) -> impl IntoResponse {
    Json(auth.list())
}

pub async fn create(State(auth): State<Arc<Auth>>, Json(new): Json<NewUser>) -> Response {
    match auth.create(&new.name) {
        Ok(created) => {
            info!(name = created.name, "token made");
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, format!("{error:#}")).into_response(),
    }
}

pub async fn revoke(State(auth): State<Arc<Auth>>, UrlPath(id): UrlPath<String>) -> Response {
    match auth.revoke(&id) {
        Ok(true) => {
            info!(id, "token revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            warn!("could not revoke a token: {error:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
        }
    }
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

fn random_hex(len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(hex(&bytes))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn save(path: &Path, users: &[User]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, &serde_json::to_vec_pretty(users)?)?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not write {}", path.display()))
}

/// Written readable by the server's user only.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("logbook-auth-{}", random_hex(8).unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_token_lets_in_until_revoked_and_survives_a_restart() {
        let dir = dir();
        let auth = Auth::open(&dir, Some("admin".into()), true).unwrap();
        let made = auth.create(" Luca ").unwrap();
        assert!(made.token.starts_with(PREFIX));
        assert_eq!(auth.check(&made.token).as_deref(), Some("Luca"));
        assert_eq!(auth.check("lbk_wrong"), None);

        let again = Auth::open(&dir, Some("admin".into()), true).unwrap();
        assert_eq!(again.check(&made.token).as_deref(), Some("Luca"));
        let kept = std::fs::read_to_string(dir.join("tokens.json")).unwrap();
        assert!(!kept.contains(&made.token), "the token itself is kept");

        assert!(again.revoke(&made.id).unwrap());
        assert_eq!(again.check(&made.token), None);
        assert!(!again.revoke(&made.id).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_admin_token_is_made_once() {
        let dir = dir();
        Auth::open(&dir, None, true).unwrap();
        let first = std::fs::read_to_string(dir.join("admin.token")).unwrap();
        Auth::open(&dir, None, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("admin.token")).unwrap(),
            first
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
