//! libparakeet and the models from the mirror on ghcr.io/brainy-org
//! (github.com/brainy-org/models), fetched on first use into a cache, as
//! brainy-worker does (its `registry.py`): never from GitHub releases or
//! Hugging Face, every file by its sha256, checked as it arrives and
//! renamed into place only once it matches. The pins below are the mirror's
//! files: the `parakeet` bundle's are brainy-worker's (`models.json`).
//!
//! Nothing outside the binary is used: no oras, docker or nvidia-smi, no
//! file another tool wrote. logbook's packages are public, so no
//! credentials either: the registry's 401 names a realm, which gives an
//! anonymous Bearer token for pulling. A blob is redirected to a CDN, which
//! gets no Authorization header.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

pub const REGISTRY: &str = "ghcr.io";
/// Bundles are repositories under this one: `brainy-org/models/parakeet`.
const REPOSITORY: &str = "brainy-org/models";

/// Model files: name, sha256, bundle. The `parakeet` bundle is the one
/// brainy-worker loads; `parakeet-voice` the speaker encoder, logbook's
/// alone. `MODELS_DIR` names the folder they go in.
pub const MODELS: [(&str, &str, &str); 4] = [
    (
        "tdt-0.6b-v3-q8_0.gguf",
        "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
        "parakeet",
    ),
    (
        "silero-vad-f16.gguf",
        "8160489282352accc0e95925c2f6bf3d76fb8f7bccdce5c7e46808cb15e8443c",
        "parakeet",
    ),
    (
        "nemotron-3-diarization-q8_0.gguf",
        "76c5bb1fb20d82706142ad32769b7ab496d2458489473a000fd7074c52ceec22",
        "parakeet",
    ),
    (
        "wespeaker-resnet34-lm.gguf",
        "72040372494eafec299836bc1977cfc13c603cb486674ed59b0f4c03758d29da",
        "parakeet-voice",
    ),
];
/// The `parakeet` bundle's files are mudler/parakeet-cpp-gguf at commit
/// 741158ae; under the cache folder.
pub const MODELS_DIR: &str = "parakeet-741158ae";

/// The `libparakeet` bundle (v0.6.1, `ABI` in parakeet.rs): a tar.gz per
/// build, with the library, its header and, for CUDA, cudart and cuBLAS.
const LIBPARAKEET: [(&str, &str); 5] = [
    (
        "linux-cpu-x64",
        "5a2b439e83d1b6ab66c55bcfc48751ee48e53727d5b3ea33a6bc0e8626f1397c",
    ),
    (
        "linux-cpu-arm64",
        "d9e56bab07f87b6c05593a039f7cfe5b6033995da80f21721f167bfb41698c42",
    ),
    (
        "linux-cuda-x64",
        "f717526c49d56cd4691390ca4c8e521a27eec698a93f3897bf3817ceb744378a",
    ),
    (
        "macos-metal-arm64",
        "1847118600359355c899619e263275d83b59f81dcc34fcca6ba4e6b9aac67db0",
    ),
    (
        "macos-cpu-x64",
        "4575814a5abb2d8d78b657750a3418cf2178765aa8f2f4ebfd692bf78fda9e7d",
    ),
];

/// `~/.cache/brainy-org`, where brainy-org's tools keep what they fetch.
pub fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".cache/brainy-org"))
}

/// This machine's libparakeet build and the library's file name: Metal on
/// Apple silicon, CUDA on Linux with NVIDIA's driver loaded, CPU otherwise.
pub fn build() -> Result<(&'static str, &'static str)> {
    // The kernel module's own file: there with the driver, whatever is
    // installed beside it. And its device: a container sees the host's
    // /proc/driver/nvidia whether or not it was given the GPU, and only
    // one given it (`--gpus`) has /dev/nvidiactl; without, the CUDA build
    // would fail to load for want of libcuda.
    let cuda = || {
        Path::new("/proc/driver/nvidia/version").is_file() && Path::new("/dev/nvidiactl").exists()
    };
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("macos-metal-arm64", "libparakeet.dylib"),
        ("macos", "x86_64") => ("macos-cpu-x64", "libparakeet.dylib"),
        ("linux", "x86_64") if cuda() => ("linux-cuda-x64", "libparakeet.so"),
        ("linux", "x86_64") => ("linux-cpu-x64", "libparakeet.so"),
        ("linux", "aarch64") => ("linux-cpu-arm64", "libparakeet.so"),
        (os, arch) => bail!("no libparakeet build for {os} {arch}"),
    })
}

pub struct Mirror {
    agent: ureq::Agent,
    /// Bearer tokens by repository.
    tokens: Mutex<HashMap<String, String>>,
}

impl Default for Mirror {
    fn default() -> Self {
        Self::new()
    }
}

impl Mirror {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// The models folder (the one given, used as it is, or the pinned one
    /// in the cache) and libparakeet for this machine (the pinned build),
    /// fetched when missing. A platform without a build gets one in the
    /// mirror, not a path of its own. On Linux
    /// the process may start again here, to find the CUDA build's
    /// libraries (`beside_its_libraries`).
    pub fn locate(
        &self,
        models: Option<PathBuf>,
        say: impl Fn(&str),
    ) -> Result<(PathBuf, PathBuf)> {
        let models = match models {
            Some(dir) => dir,
            None => {
                let dir = cache_dir()?.join(MODELS_DIR);
                self.models(&dir, &say)?;
                dir
            }
        };
        let library = self.library(&cache_dir()?, &say)?;
        #[cfg(target_os = "linux")]
        beside_its_libraries(&library)?;
        Ok((models, library))
    }

    /// The model files missing from `dir`, fetched. `say` hears what is
    /// being fetched. A file there is taken as complete: one is only ever
    /// renamed into place once its sha256 matched.
    pub fn models(&self, dir: &Path, say: impl Fn(&str)) -> Result<()> {
        for (file, sha256, bundle) in MODELS {
            let path = dir.join(file);
            if path.is_file() {
                continue;
            }
            say(&format!(
                "fetching {file} from {REGISTRY}/{REPOSITORY}/{bundle}…"
            ));
            fs::create_dir_all(dir)?;
            self.blob(bundle, sha256, &path)?;
        }
        Ok(())
    }

    /// libparakeet for this machine under `cache`, fetched if missing; its
    /// path. Each build and release has a folder of its own.
    pub fn library(&self, cache: &Path, say: impl Fn(&str)) -> Result<PathBuf> {
        let (build, file) = build()?;
        let sha256 = LIBPARAKEET
            .iter()
            .find(|(b, _)| *b == build)
            .map(|(_, s)| *s)
            .ok_or_else(|| anyhow!("no libparakeet pinned for {build}"))?;
        let dir = cache.join(format!("libparakeet-{build}-{}", &sha256[..12]));
        let path = dir.join(file);
        if path.is_file() {
            return Ok(path);
        }
        say(&format!(
            "fetching libparakeet {build} from {REGISTRY}/{REPOSITORY}/libparakeet…"
        ));
        fs::create_dir_all(cache)?;
        let partial = cache.join(format!(".libparakeet-{build}.partial"));
        let _ = fs::remove_dir_all(&partial);
        fs::create_dir_all(&partial)?;
        let archive = partial.join("lib.tar.gz");
        self.blob("libparakeet", sha256, &archive)?;
        unpack(&archive, &partial)?;
        fs::remove_file(&archive)?;
        if !partial.join(file).is_file() {
            bail!("the libparakeet {build} archive has no {file}");
        }
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&partial, &dir)?;
        Ok(path)
    }

    /// The blob `sha256` of `bundle` into `path`: written beside it, hashed
    /// as it arrives, renamed into place once it matches.
    fn blob(&self, bundle: &str, sha256: &str, path: &Path) -> Result<()> {
        let repository = format!("{REPOSITORY}/{bundle}");
        let url = format!("https://{REGISTRY}/v2/{repository}/blobs/sha256:{sha256}");
        let mut response = self.get(&repository, &url)?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("blob");
        let partial = path.with_file_name(format!(".{name}.partial"));
        let mut out = File::create(&partial)
            .with_context(|| format!("could not write {}", partial.display()))?;
        let mut hasher = Sha256::new();
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 20];
        let copied = (|| -> Result<()> {
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
            }
        })();
        drop(out);
        if let Err(error) = copied {
            let _ = fs::remove_file(&partial);
            return Err(error.context(format!("fetching {repository}@sha256:{sha256}")));
        }
        let got = hex(&hasher.finalize());
        if got != sha256 {
            let _ = fs::remove_file(&partial);
            bail!("{repository}: the blob is sha256:{got}, not sha256:{sha256}");
        }
        fs::rename(&partial, path)?;
        Ok(())
    }

    /// A GET on the registry, with a Bearer token for `repository` once it
    /// asked for one.
    fn get(&self, repository: &str, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut retried = false;
        loop {
            let token = self.tokens.lock().unwrap().get(repository).cloned();
            let mut request = self.agent.get(url);
            if let Some(token) = &token {
                request = request.header("Authorization", &format!("Bearer {token}"));
            }
            let response = request
                .call()
                .with_context(|| format!("could not reach {REGISTRY}"))?;
            let status = response.status();
            if status.as_u16() == 401 && !retried {
                retried = true;
                let challenge = response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let token = self.token(&challenge, repository)?;
                self.tokens.lock().unwrap().insert(repository.into(), token);
                continue;
            }
            if !status.is_success() {
                bail!("{REGISTRY}/{repository}: {url} answered {status}");
            }
            return Ok(response);
        }
    }

    /// A Bearer token from the realm of the registry's challenge.
    fn token(&self, challenge: &str, repository: &str) -> Result<String> {
        let Some(params) = challenge
            .strip_prefix("Bearer ")
            .or_else(|| challenge.strip_prefix("bearer "))
        else {
            bail!("{REGISTRY}: authentication required ({challenge:?})");
        };
        let mut fields: HashMap<String, String> = params
            .split(',')
            .filter_map(|f| f.trim().split_once('='))
            .map(|(k, v)| (k.to_string(), v.trim_matches('"').to_string()))
            .collect();
        let realm = fields
            .remove("realm")
            .ok_or_else(|| anyhow!("{REGISTRY}: a Bearer challenge without a realm"))?;
        fields
            .entry("scope".into())
            .or_insert_with(|| format!("repository:{repository}:pull"));
        let mut request = self.agent.get(&realm);
        for (k, v) in &fields {
            request = request.query(k, v);
        }
        let mut response = request
            .call()
            .with_context(|| format!("could not reach {realm}"))?;
        if !response.status().is_success() {
            bail!(
                "{REGISTRY}: a token for {repository} was refused ({}): is the package \
                 still public?",
                response.status()
            );
        }
        let body: serde_json::Value = response.body_mut().read_json()?;
        body["token"]
            .as_str()
            .or_else(|| body["access_token"].as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("{REGISTRY}: no token in the answer"))
    }
}

/// The archive into `dir`, without its top folder (`tar --strip-components=1`).
/// Paths that would leave `dir` are refused.
fn unpack(archive: &Path, dir: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let inner: PathBuf = path.components().skip(1).collect();
        if inner.as_os_str().is_empty() {
            continue;
        }
        if !inner
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            bail!("the archive has an unsafe path: {}", path.display());
        }
        let target = dir.join(&inner);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The CUDA build ships cudart and cuBLAS next to libparakeet, which the
/// loader finds only through LD_LIBRARY_PATH, read when the process
/// starts: the process (server or CLI) starts again with that folder in it.
#[cfg(target_os = "linux")]
fn beside_its_libraries(library: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let Some(dir) = library.parent() else {
        return Ok(());
    };
    let others = std::fs::read_dir(dir)?.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.contains(".so") && !name.starts_with("libparakeet")
    });
    let path = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    if !others || std::env::split_paths(&path).any(|p| p == dir) {
        return Ok(());
    }
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&path));
    let error = std::process::Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .env("LD_LIBRARY_PATH", std::env::join_paths(paths)?)
        .exec();
    Err(error).context("could not start again with libparakeet's libraries")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_loses_its_top_folder_and_keeps_inside() {
        let dir = std::env::temp_dir().join(format!("logbook-unpack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("a.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "libparakeet-x/libparakeet.so", &b"abc"[..])
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        unpack(&archive, &out).unwrap();
        assert_eq!(fs::read(out.join("libparakeet.so")).unwrap(), b"abc");
        fs::remove_dir_all(&dir).unwrap();
    }
}
