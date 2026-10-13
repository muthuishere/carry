//! Shared tool store: publish/promote/yank/ls, apps.lock edits, and the
//! busybox-style launcher used when carry runs under a tool's name.
//! See docs/adr/0006-tools-on-demand-from-a-shared-store.md.
#![allow(dead_code)]
use crate::cli::ToolSource;
use crate::crypto;
use crate::ctx;
use crate::error::{CarryError, Result};
use crate::model::{SignedToolMeta, ToolBuild, ToolMeta, ToolRef};
use crate::repo::Repo;
use crate::settings::Settings;
use anyhow::anyhow;
use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const RMW_RETRIES: usize = 5;

type Channels = BTreeMap<String, String>;

fn meta_key(name: &str, version: &str) -> String {
    format!("tools/{name}/{version}/tool.json")
}
fn channels_key(name: &str) -> String {
    format!("tools/{name}/channels.json")
}

fn valid_name(what: &str, s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s != "."
        && s != ".."
        && s != "blobs"
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
    if ok {
        Ok(())
    } else {
        Err(CarryError::Usage(format!("bad {what} '{s}'")))
    }
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

// ---------------------------------------------------------------- signing

pub fn sign_meta(meta: ToolMeta, secret_b64: &str) -> Result<SignedToolMeta> {
    let msg = serde_json::to_vec(&meta).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    Ok(SignedToolMeta {
        signature: crypto::sign(secret_b64, &msg)?,
        signer: crypto::ed25519_public(secret_b64)?,
        meta,
    })
}

/// Verify the envelope's signature AND that the signer is `trusted`.
pub fn verify_meta(s: &SignedToolMeta, trusted: &str) -> Result<()> {
    if s.signer.trim() != trusted.trim() {
        return Err(CarryError::Invalid(format!(
            "{}@{} is signed by an untrusted key",
            s.meta.name, s.meta.version
        )));
    }
    let msg = serde_json::to_vec(&s.meta).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    crypto::verify(trusted, &msg, &s.signature)
        .map_err(|_| CarryError::Invalid(format!("{}@{}: bad signature", s.meta.name, s.meta.version)))
}

fn signing_key(s: &Settings) -> Result<String> {
    s.signing_key
        .clone()
        .ok_or_else(|| CarryError::BadKey("no signing key (set CARRY_TOOL_SIGNING_KEY or CARRY_TOOL_SIGNING_KEY_FILE)".into()))
}

// ---------------------------------------------------------------- JSON read-modify-write with If-Match

async fn read_json<T: serde::de::DeserializeOwned>(repo: &Repo, key: &str) -> Result<Option<(T, String)>> {
    match repo.store.get_with_etag(key).await? {
        None => Ok(None),
        Some((b, etag)) => {
            let v = serde_json::from_slice(&b).map_err(|e| CarryError::Invalid(format!("{key}: {e}")))?;
            Ok(Some((v, etag)))
        }
    }
}

async fn write_json<T: serde::Serialize>(repo: &Repo, key: &str, v: &T, etag: Option<&str>) -> Result<bool> {
    let b = Bytes::from(serde_json::to_vec_pretty(v).map_err(|e| CarryError::Internal(anyhow!(e)))?);
    match etag {
        None => repo.store.put_if_absent(key, b).await,
        // Some stores return no etag; fall back to a plain put.
        Some("") => repo.store.put(key, b).await.map(|_| true),
        Some(e) => match repo.store.put_if_match(key, b, e).await {
            Ok(_) => Ok(true),
            Err(CarryError::Conflict(_)) => Ok(false),
            Err(x) => Err(x),
        },
    }
}

/// Read-modify-write a JSON document; retries when another writer got there first.
async fn rmw<T, F>(repo: &Repo, key: &str, mut f: F) -> Result<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
    F: FnMut(Option<T>) -> Result<T>,
{
    for _ in 0..RMW_RETRIES {
        let cur = read_json::<T>(repo, key).await?;
        let (old, etag) = match cur {
            Some((v, e)) => (Some(v), Some(e)),
            None => (None, None),
        };
        let next = f(old)?;
        if write_json(repo, key, &next, etag.as_deref()).await? {
            return Ok(next);
        }
        tokio::time::sleep(std::time::Duration::from_millis(30 + rand::random::<u64>() % 100)).await;
    }
    Err(CarryError::Conflict(format!("{key} changed concurrently")))
}

// ---------------------------------------------------------------- commands

pub async fn keygen(_s: &Settings, out: Option<&Path>) -> Result<Value> {
    let out = match out {
        Some(p) => p.to_path_buf(),
        None => dirs::home_dir()
            .map(|h| h.join(".config/carry/tool-signing.key"))
            .ok_or_else(|| CarryError::Usage("cannot determine key file path".into()))?,
    };
    keygen_to(&out)
}

pub fn keygen_to(out: &Path) -> Result<Value> {
    if out.exists() {
        return Err(CarryError::Usage(format!("{} already exists; refusing to overwrite", out.display())));
    }
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| CarryError::Usage(format!("cannot create {}: {e}", p.display())))?;
    }
    let (secret, public) = crypto::ed25519_keygen();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out)
        .map_err(|e| CarryError::Usage(format!("cannot write {}: {e}", out.display())))?;
    f.write_all(format!("{secret}\n").as_bytes())
        .map_err(|e| CarryError::Usage(format!("cannot write {}: {e}", out.display())))?;
    Ok(json!({"public_key": public, "private_key_file": out.display().to_string()}))
}

async fn download(url: &str) -> Result<Vec<u8>> {
    use futures::StreamExt;
    let resp = reqwest::get(url)
        .await
        .map_err(|e| CarryError::Unreachable(format!("{url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(CarryError::NotFound(format!("{url}: HTTP {}", resp.status())));
    }
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(c) = s.next().await {
        out.extend_from_slice(&c.map_err(|e| CarryError::Unreachable(format!("{url}: {e}")))?);
    }
    Ok(out)
}

pub async fn publish(s: &Settings, name: &str, version: &str, src: ToolSource, platform: &str) -> Result<Value> {
    valid_name("tool name", name)?;
    valid_name("version", version)?;
    if !platform.contains('/') {
        return Err(CarryError::Usage(format!("platform must be os/arch, got '{platform}'")));
    }
    let sk = signing_key(s)?;
    let me = crypto::ed25519_public(&sk)?;
    let repo = ctx::open_repo_for_write(s).await?;
    let (sha, size, blob) = match src {
        ToolSource::File(p) => {
            // Stream from disk: sha256 pass here, then the repo hashes and uploads in bounded chunks.
            let ue = |e: std::io::Error| CarryError::Usage(format!("cannot read {}: {e}", p.display()));
            let mut f = std::fs::File::open(&p).map_err(ue)?;
            let size = f.metadata().map_err(ue)?.len();
            let mut h = Sha256::new();
            std::io::copy(&mut f, &mut ShaSink(&mut h)).map_err(ue)?;
            let sha = hex::encode(h.finalize());
            let (blob, _) = repo.put_tool_blob_from(f, size).await?;
            (sha, size, blob)
        }
        ToolSource::Url { url, sha256 } => {
            let b = download(&url).await?;
            if let Some(want) = sha256 {
                let got = sha256_hex(&b);
                if !got.eq_ignore_ascii_case(want.trim()) {
                    return Err(CarryError::Invalid(format!("sha256 mismatch for {url}: got {got}")));
                }
            }
            (sha256_hex(&b), b.len() as u64, repo.put_tool_blob(&b).await?)
        }
    };
    let build = ToolBuild { sha256: sha.clone(), size, blob };
    rmw::<SignedToolMeta, _>(&repo, &meta_key(name, version), |old| {
        let meta = match old {
            Some(old) => {
                if old.signer.trim() != me {
                    return Err(CarryError::Refused(format!("{name}@{version} was signed by a different key")));
                }
                verify_meta(&old, &me)?;
                let mut m = old.meta;
                m.platforms.insert(platform.to_string(), build.clone());
                m
            }
            None => ToolMeta {
                name: name.into(),
                version: version.into(),
                platforms: BTreeMap::from([(platform.to_string(), build.clone())]),
                yanked: false,
                published_at: chrono::Utc::now(),
            },
        };
        sign_meta(meta, &sk)
    })
    .await?;
    Ok(json!({"name": name, "version": version, "platform": platform, "sha256": sha}))
}

pub async fn promote(s: &Settings, name: &str, version: &str, channel: &str) -> Result<Value> {
    valid_name("channel", channel)?;
    let repo = ctx::open_repo_for_write(s).await?;
    let m: SignedToolMeta = repo
        .get_tool_json(&meta_key(name, version))
        .await?
        .ok_or_else(|| CarryError::NotFound(format!("{name}@{version}")))?;
    if m.meta.yanked {
        return Err(CarryError::Refused(format!("{name}@{version} is yanked")));
    }
    rmw::<Channels, _>(&repo, &channels_key(name), |old| {
        let mut c = old.unwrap_or_default();
        c.insert(channel.into(), version.into());
        Ok(c)
    })
    .await?;
    Ok(json!({"name": name, "channel": channel, "version": version}))
}

pub async fn yank(s: &Settings, name: &str, version: &str) -> Result<Value> {
    let sk = signing_key(s)?;
    let me = crypto::ed25519_public(&sk)?;
    let repo = ctx::open_repo_for_write(s).await?;
    rmw::<SignedToolMeta, _>(&repo, &meta_key(name, version), |old| {
        let old = old.ok_or_else(|| CarryError::NotFound(format!("{name}@{version}")))?;
        verify_meta(&old, &me).map_err(|_| CarryError::Refused(format!("{name}@{version} was signed by a different key")))?;
        let mut m = old.meta;
        m.yanked = true;
        sign_meta(m, &sk)
    })
    .await?;
    Ok(json!({"name": name, "version": version, "yanked": true}))
}

pub async fn ls(s: &Settings, name: Option<&str>) -> Result<Value> {
    let repo = ctx::open_repo(s).await?;
    let names = match name {
        Some(n) => vec![n.to_string()],
        None => repo.store.list_dirs("tools").await?.into_iter().filter(|n| n != "blobs").collect(),
    };
    let mut tools = Vec::new();
    for n in names {
        let versions = repo.store.list_dirs(&format!("tools/{n}")).await?;
        let channels: Channels = repo.get_tool_json(&channels_key(&n)).await?.unwrap_or_default();
        if name.is_some() && versions.is_empty() {
            return Err(CarryError::NotFound(format!("tool '{n}'")));
        }
        tools.push(json!({"name": n, "versions": versions, "channels": channels}));
    }
    Ok(json!({"tools": tools}))
}

pub async fn add(s: &Settings, state: &str, name: &str, r: ToolRef) -> Result<Value> {
    valid_name("tool name", name)?;
    let repo = ctx::open_repo_for_write(s).await?;
    if let Some(v) = &r.version {
        if repo.get_tool_json::<SignedToolMeta>(&meta_key(name, v)).await?.is_none() {
            return Err(CarryError::NotFound(format!("{name}@{v}")));
        }
    }
    let args = vec![name.to_string()];
    let head = ctx::mutate(&repo, state, "tool_add", args, |mut m| {
        let r = r.clone();
        let name = name.to_string();
        async move {
            m.apps.insert(name, r);
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "version": head.seq, "tool": name}))
}

pub async fn rm(s: &Settings, state: &str, name: &str) -> Result<Value> {
    let repo = ctx::open_repo_for_write(s).await?;
    let head = ctx::mutate(&repo, state, "tool_rm", vec![name.to_string()], |mut m| {
        let name = name.to_string();
        let state = state.to_string();
        async move {
            if m.apps.remove(&name).is_none() {
                return Err(CarryError::NotFound(format!("tool '{name}' in state '{state}'")));
            }
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "version": head.seq, "tool": name}))
}

// ---------------------------------------------------------------- links

/// Make `<root>/bin/<name>` a symlink to the carry executable for every listed
/// tool, and remove links for tools no longer listed.
pub fn link_tools(root: &Path, apps: &BTreeMap<String, ToolRef>) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    link_tools_to(root, apps, &exe)
}

pub fn link_tools_to(root: &Path, apps: &BTreeMap<String, ToolRef>, exe: &Path) -> std::io::Result<()> {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin)?;
    for e in std::fs::read_dir(&bin)? {
        let e = e?;
        let n = e.file_name().to_string_lossy().to_string();
        let is_link = e.file_type()?.is_symlink();
        if is_link && !apps.contains_key(&n) {
            std::fs::remove_file(e.path())?;
        }
    }
    for name in apps.keys() {
        if valid_name("tool name", name).is_err() {
            continue;
        }
        let p = bin.join(name);
        if std::fs::read_link(&p).ok().as_deref() == Some(exe) {
            continue;
        }
        let tmp = bin.join(format!(".{name}.tmp{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(exe, &tmp)?;
        std::fs::rename(&tmp, &p)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- launcher

pub fn current_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}/{arch}")
}

/// Read apps from `<root>/current/manifest.json`.
pub fn read_apps(root: &Path) -> std::result::Result<BTreeMap<String, ToolRef>, String> {
    let p = root.join("current/manifest.json");
    let b = std::fs::read(&p).map_err(|e| format!("no restored state ({}: {e})", p.display()))?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| format!("bad {}: {e}", p.display()))?;
    let apps = v.get("apps").cloned().unwrap_or(Value::Null);
    if apps.is_null() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_value(apps).map_err(|e| format!("bad apps in {}: {e}", p.display()))
}

/// Pick the version a ref points at. Channel lookups go through `channels`.
pub fn resolve_version(name: &str, r: &ToolRef, channels: Option<&Channels>) -> std::result::Result<String, String> {
    if let Some(v) = &r.version {
        return Ok(v.clone());
    }
    let c = r.channel.as_deref().unwrap_or("stable");
    channels
        .and_then(|m| m.get(c))
        .cloned()
        .ok_or_else(|| format!("channel '{c}' has no version"))
        .map_err(|e| format!("{e} (tool {name})"))
}

/// Check a fetched tool.json and pick the build for `platform`.
pub fn select_build(m: &SignedToolMeta, pubkey: &str, platform: &str) -> std::result::Result<ToolBuild, String> {
    verify_meta(m, pubkey).map_err(|e| format!("refused: {e}"))?;
    if m.meta.yanked {
        return Err(format!("{}@{} is yanked", m.meta.name, m.meta.version));
    }
    m.meta
        .platforms
        .get(platform)
        .cloned()
        .ok_or_else(|| format!("{}@{} has no build for {platform}", m.meta.name, m.meta.version))
}

fn cache_dir(root: &Path) -> PathBuf {
    root.join("cache/tools")
}

/// Entry point when carry runs under a tool's name. Returns an exit code only on failure.
pub fn launch_from_argv0(name: &str) -> i32 {
    match prepare(name) {
        Ok(bin) => {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&bin).arg0(name).args(std::env::args_os().skip(1)).exec();
            eprintln!("carry: {name}: exec {}: {err}", bin.display());
            127
        }
        Err(e) => {
            eprintln!("carry: {name}: {e}");
            127
        }
    }
}

fn prepare(name: &str) -> std::result::Result<PathBuf, String> {
    let root = std::env::var("CARRY_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/carry"));
    let apps = read_apps(&root)?;
    let r = apps.get(name).ok_or_else(|| "not listed in this state's apps".to_string())?;
    let pubkey = std::env::var("CARRY_TOOL_PUBKEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "refused: CARRY_TOOL_PUBKEY is not set".to_string())?;
    let platform = current_platform();
    let meta_cache = cache_dir(&root).join("meta").join(name);

    // Fast path: pinned version with verified metadata and binary already cached.
    if let Some(v) = &r.version {
        let mf = meta_cache.join(format!("{v}.json"));
        if let Some(bin) = cached_binary(&root, &mf, &pubkey, &platform) {
            // Best effort: honour a yank if the store answers within 1 s; offline -> run cached.
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
            if let Ok(rt) = rt {
                let fetch_meta = async {
                    let s = Settings::resolve(&crate::cli::GlobalArgs::default())?;
                    let repo = ctx::open_repo(&s).await?;
                    repo.get_tool_json::<SignedToolMeta>(&meta_key(name, v)).await
                };
                let fresh = rt.block_on(yank_check(fetch_meta, &pubkey, YANK_CHECK_BUDGET));
                rt.shutdown_background();
                if let Some(m) = fresh.filter(|m| m.meta.yanked) {
                    // Persist so later offline launches refuse too.
                    if let Ok(b) = serde_json::to_vec(&m) {
                        let _ = std::fs::write(&mf, b);
                    }
                    return Err(format!("refused: {name}@{v} is yanked"));
                }
            }
            return Ok(bin);
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    rt.block_on(fetch(name, r, &root, &pubkey, &platform, &meta_cache))
}

const YANK_CHECK_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

/// Fetch fresh metadata within `budget`. Returns it only if it arrived in time and is
/// signed by `pubkey`; any error, timeout or bad signature yields None (keep the cache).
async fn yank_check<F>(fetch: F, pubkey: &str, budget: std::time::Duration) -> Option<SignedToolMeta>
where
    F: std::future::Future<Output = Result<Option<SignedToolMeta>>>,
{
    let m = tokio::time::timeout(budget, fetch).await.ok()?.ok()??;
    verify_meta(&m, pubkey).ok()?;
    Some(m)
}

fn cached_binary(root: &Path, meta_file: &Path, pubkey: &str, platform: &str) -> Option<PathBuf> {
    let b = std::fs::read(meta_file).ok()?;
    let m: SignedToolMeta = serde_json::from_slice(&b).ok()?;
    let build = select_build(&m, pubkey, platform).ok()?;
    let bin = cache_dir(root).join(&build.sha256);
    bin.is_file().then_some(bin)
}

async fn fetch(
    name: &str,
    r: &ToolRef,
    root: &Path,
    pubkey: &str,
    platform: &str,
    meta_cache: &Path,
) -> std::result::Result<PathBuf, String> {
    let s = Settings::resolve(&crate::cli::GlobalArgs::default()).map_err(|e| e.to_string())?;
    let repo = ctx::open_repo(&s).await.map_err(|e| e.to_string())?;
    let version = if r.version.is_some() {
        resolve_version(name, r, None)?
    } else {
        let ch: Option<Channels> = repo.get_tool_json(&channels_key(name)).await.map_err(|e| e.to_string())?;
        resolve_version(name, r, ch.as_ref())?
    };
    let m: SignedToolMeta = repo
        .get_tool_json(&meta_key(name, &version))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{name}@{version} not found in the tool store"))?;
    let build = select_build(&m, pubkey, platform)?;
    let dir = cache_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bin = dir.join(&build.sha256);

    // One downloader per tool; others wait, then find the binary present.
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(format!(".lock-{name}")))
        .map_err(|e| format!("lock: {e}"))?;
    let _lock = nix::fcntl::Flock::lock(lock_file, nix::fcntl::FlockArg::LockExclusive)
        .map_err(|(_, e)| format!("lock: {e}"))?;

    if !bin.is_file() {
        let tmp = dir.join(format!(".{}.tmp{}", build.sha256, std::process::id()));
        let got = download_blob(&repo, &build.blob, &tmp).await;
        let got = match got {
            Ok(g) => g,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if got != build.sha256 {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("refused: sha256 mismatch (want {}, got {got})", build.sha256));
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("chmod: {e}"))?;
        std::fs::rename(&tmp, &bin).map_err(|e| format!("cache rename: {e}"))?;
    }
    // Remember verified metadata so later pinned launches need no network.
    if std::fs::create_dir_all(meta_cache).is_ok() {
        if let Ok(b) = serde_json::to_vec(&m) {
            let f = meta_cache.join(format!("{version}.json"));
            let tmp = meta_cache.join(format!(".{version}.tmp{}", std::process::id()));
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, f);
            }
        }
    }
    Ok(bin)
}

struct ShaSink<'a>(&'a mut Sha256);
impl std::io::Write for ShaSink<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Writer that hashes (SHA-256) everything passing through.
struct Sha256Writer<W> {
    inner: W,
    h: sha2::Sha256,
}

impl<W: std::io::Write> std::io::Write for Sha256Writer<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest;
        let n = self.inner.write(b)?;
        self.h.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Stream a tool blob to `dest` with bounded memory; returns its sha256 hex.
async fn download_blob(repo: &Repo, blob: &str, dest: &Path) -> std::result::Result<String, String> {
    use sha2::Digest;
    let f = std::fs::File::create(dest).map_err(|e| format!("cache write: {e}"))?;
    let mut w = Sha256Writer { inner: std::io::BufWriter::with_capacity(256 << 10, f), h: sha2::Sha256::new() };
    repo.get_tool_blob_to(blob, &mut w).await.map_err(|e| e.to_string())?;
    let f = w.inner.into_inner().map_err(|e| format!("cache write: {}", e.error()))?;
    f.sync_all().map_err(|e| format!("cache write: {e}"))?;
    Ok(hex::encode(w.h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(yanked: bool) -> ToolMeta {
        ToolMeta {
            name: "jq".into(),
            version: "1.7".into(),
            platforms: BTreeMap::from([(
                "linux/amd64".into(),
                ToolBuild { sha256: "ab".into(), size: 2, blob: "b1".into() },
            )]),
            yanked,
            published_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn sign_and_verify() {
        let (sk, pk) = crypto::ed25519_keygen();
        let s = sign_meta(meta(false), &sk).unwrap();
        assert_eq!(s.signer, pk);
        verify_meta(&s, &pk).unwrap();
        // Round-trip through JSON still verifies.
        let back: SignedToolMeta = serde_json::from_str(&serde_json::to_string_pretty(&s).unwrap()).unwrap();
        verify_meta(&back, &pk).unwrap();
        // Tampering breaks it.
        let mut t = back.clone();
        t.meta.platforms.get_mut("linux/amd64").unwrap().sha256 = "cd".into();
        assert!(verify_meta(&t, &pk).is_err());
        // Another key is refused.
        let (_, other) = crypto::ed25519_keygen();
        assert!(verify_meta(&s, &other).is_err());
    }

    #[test]
    fn refuses_unsigned_or_foreign() {
        let (sk, pk) = crypto::ed25519_keygen();
        let mut s = sign_meta(meta(false), &sk).unwrap();
        s.signature = String::new();
        assert!(select_build(&s, &pk, "linux/amd64").unwrap_err().starts_with("refused"));
        let (sk2, _) = crypto::ed25519_keygen();
        let foreign = sign_meta(meta(false), &sk2).unwrap();
        assert!(select_build(&foreign, &pk, "linux/amd64").unwrap_err().starts_with("refused"));
    }

    #[test]
    fn yank_and_channels() {
        let (sk, pk) = crypto::ed25519_keygen();
        let ok = sign_meta(meta(false), &sk).unwrap();
        assert_eq!(select_build(&ok, &pk, "linux/amd64").unwrap().sha256, "ab");
        assert!(select_build(&ok, &pk, "linux/arm64").unwrap_err().contains("no build"));
        let y = sign_meta(meta(true), &sk).unwrap();
        assert!(select_build(&y, &pk, "linux/amd64").unwrap_err().contains("yanked"));

        let ch = Channels::from([("stable".into(), "1.7".into())]);
        let pinned = ToolRef { version: Some("1.6".into()), ..Default::default() };
        assert_eq!(resolve_version("jq", &pinned, Some(&ch)).unwrap(), "1.6");
        let dflt = ToolRef::default();
        assert_eq!(resolve_version("jq", &dflt, Some(&ch)).unwrap(), "1.7");
        let beta = ToolRef { channel: Some("beta".into()), ..Default::default() };
        assert!(resolve_version("jq", &beta, Some(&ch)).is_err());
    }

    #[tokio::test]
    async fn yank_check_is_best_effort() {
        let (sk, pk) = crypto::ed25519_keygen();
        let y = sign_meta(meta(true), &sk).unwrap();
        let b = std::time::Duration::from_secs(1);
        let got = yank_check(async { Ok(Some(y.clone())) }, &pk, b).await;
        assert!(got.unwrap().meta.yanked);
        // Unreachable store: silently skipped, within the budget.
        let t = std::time::Instant::now();
        let slow = async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(Some(y.clone()))
        };
        assert!(yank_check(slow, &pk, b).await.is_none());
        assert!(t.elapsed() < std::time::Duration::from_millis(1500));
        assert!(yank_check(async { Err(CarryError::Unreachable("down".into())) }, &pk, b).await.is_none());
        // Forged metadata cannot yank (or un-yank) anything.
        let (_, other) = crypto::ed25519_keygen();
        assert!(yank_check(async { Ok(Some(y.clone())) }, &other, b).await.is_none());
    }

    #[test]
    fn cache_hit_needs_no_network() {
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        let plat = current_platform();
        let body = b"#!/bin/sh\necho hi\n";
        let sha = sha256_hex(body);
        let mut m = meta(false);
        m.platforms = BTreeMap::from([(plat.clone(), ToolBuild { sha256: sha.clone(), size: 3, blob: "x".into() })]);
        let s = sign_meta(m, &sk).unwrap();
        let mf = d.path().join("cache/tools/meta/jq/1.7.json");
        std::fs::create_dir_all(mf.parent().unwrap()).unwrap();
        std::fs::write(&mf, serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(cached_binary(d.path(), &mf, &pk, &plat).is_none());
        std::fs::write(d.path().join("cache/tools").join(&sha), body).unwrap();
        assert_eq!(cached_binary(d.path(), &mf, &pk, &plat).unwrap(), d.path().join("cache/tools").join(&sha));
        let (_, other) = crypto::ed25519_keygen();
        assert!(cached_binary(d.path(), &mf, &other, &plat).is_none());
    }

    #[test]
    fn links_added_and_removed() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("carry");
        std::fs::write(&exe, b"").unwrap();
        let mut apps = BTreeMap::new();
        apps.insert("jq".to_string(), ToolRef::default());
        apps.insert("yq".to_string(), ToolRef::default());
        link_tools_to(d.path(), &apps, &exe).unwrap();
        assert_eq!(std::fs::read_link(d.path().join("bin/jq")).unwrap(), exe);
        assert_eq!(std::fs::read_link(d.path().join("bin/yq")).unwrap(), exe);
        apps.remove("yq");
        link_tools_to(d.path(), &apps, &exe).unwrap();
        assert!(std::fs::symlink_metadata(d.path().join("bin/yq")).is_err());
        assert!(d.path().join("bin/jq").exists());
    }

    #[test]
    fn keygen_writes_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("k/sign.key");
        let v = keygen_to(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let secret = std::fs::read_to_string(&p).unwrap();
        assert_eq!(crypto::ed25519_public(&secret).unwrap(), v["public_key"].as_str().unwrap());
        assert!(!v.to_string().contains(secret.trim()));
        assert!(keygen_to(&p).is_err());
    }

    #[test]
    fn apps_from_manifest() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("current")).unwrap();
        std::fs::write(d.path().join("current/manifest.json"), r#"{"apps":{"jq":{"version":"1.7"}}}"#).unwrap();
        let a = read_apps(d.path()).unwrap();
        assert_eq!(a["jq"].version.as_deref(), Some("1.7"));
    }
}
