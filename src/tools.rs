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
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
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
    crypto::verify(trusted, &msg, &s.signature).map_err(|_| {
        CarryError::Invalid(format!("{}@{}: bad signature", s.meta.name, s.meta.version))
    })
}

/// Signed payload stored at tools/<name>/channels.json.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelsDoc {
    pub name: String,
    pub seq: u64,
    pub channels: Channels,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignedChannels {
    pub payload: ChannelsDoc,
    /// base64 Ed25519 signature over serde_json::to_vec(&payload)
    pub signature: String,
    pub signer: String,
}

pub fn sign_channels(payload: ChannelsDoc, secret_b64: &str) -> Result<SignedChannels> {
    let msg = serde_json::to_vec(&payload).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    Ok(SignedChannels {
        signature: crypto::sign(secret_b64, &msg)?,
        signer: crypto::ed25519_public(secret_b64)?,
        payload,
    })
}

pub fn verify_channels(s: &SignedChannels, trusted: &str) -> Result<()> {
    let n = &s.payload.name;
    if s.signer.trim() != trusted.trim() {
        return Err(CarryError::Invalid(format!(
            "{n} channels are signed by an untrusted key"
        )));
    }
    let msg = serde_json::to_vec(&s.payload).map_err(|e| CarryError::Internal(anyhow!(e)))?;
    crypto::verify(trusted, &msg, &s.signature)
        .map_err(|_| CarryError::Invalid(format!("{n} channels: bad signature")))
}

/// Parse a channels.json the launcher fetched: must be signed by `pubkey` and be for `name`.
pub fn check_channels(
    raw: &Value,
    name: &str,
    pubkey: &str,
) -> std::result::Result<ChannelsDoc, String> {
    let s: SignedChannels = serde_json::from_value(raw.clone())
        .map_err(|_| format!("refused: {name} channels.json is unsigned"))?;
    verify_channels(&s, pubkey).map_err(|e| format!("refused: {e}"))?;
    if s.payload.name != name {
        return Err(format!(
            "refused: channels.json is for '{}', not '{name}'",
            s.payload.name
        ));
    }
    Ok(s.payload)
}

/// Channel map from either a signed doc or a legacy unsigned map (display only).
fn channels_for_display(v: &Value) -> Channels {
    serde_json::from_value::<SignedChannels>(v.clone())
        .map(|s| s.payload.channels)
        .or_else(|_| serde_json::from_value::<Channels>(v.clone()))
        .unwrap_or_default()
}

// ---------------------------------------------------------------- local high-water marks (anti-replay)

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Hwm {
    #[serde(default)]
    channels: u64,
    /// version -> highest seq seen / yanked ever seen
    #[serde(default)]
    versions: BTreeMap<String, HwmEntry>,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct HwmEntry {
    seq: u64,
    yanked: bool,
}

fn hwm_path(root: &Path, name: &str) -> PathBuf {
    cache_dir(root).join("meta").join(name).join("hwm.json")
}

fn load_hwm(root: &Path, name: &str) -> Hwm {
    std::fs::read(hwm_path(root, name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_hwm(root: &Path, name: &str, h: &Hwm) -> std::result::Result<(), String> {
    let p = hwm_path(root, name);
    let dir = p.parent().expect("hwm has parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".hwm.tmp{}", std::process::id()));
    let b = serde_json::to_vec(h).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, b).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Refuse tool metadata older than what this machine already saw; remember yanks forever.
fn check_meta_hwm(root: &Path, m: &ToolMeta) -> std::result::Result<(), String> {
    let mut h = load_hwm(root, &m.name);
    let seen = h.versions.get(&m.version).cloned().unwrap_or_default();
    if m.seq < seen.seq {
        return Err(format!(
            "refused: {}@{} metadata seq {} is older than seq {} already seen (replay?)",
            m.name, m.version, m.seq, seen.seq
        ));
    }
    if seen.yanked && !m.yanked {
        return Err(format!("refused: {}@{} is yanked", m.name, m.version));
    }
    if m.seq > seen.seq || (m.yanked && !seen.yanked) {
        h.versions.insert(
            m.version.clone(),
            HwmEntry {
                seq: m.seq,
                yanked: m.yanked || seen.yanked,
            },
        );
        save_hwm(root, &m.name, &h)?;
    }
    Ok(())
}

fn locally_yanked(root: &Path, name: &str, version: &str) -> bool {
    load_hwm(root, name)
        .versions
        .get(version)
        .is_some_and(|e| e.yanked)
}

fn check_channels_hwm(root: &Path, c: &ChannelsDoc) -> std::result::Result<(), String> {
    let mut h = load_hwm(root, &c.name);
    if c.seq < h.channels {
        return Err(format!(
            "refused: {} channels seq {} is older than seq {} already seen (replay?)",
            c.name, c.seq, h.channels
        ));
    }
    if c.seq > h.channels {
        h.channels = c.seq;
        save_hwm(root, &c.name, &h)?;
    }
    Ok(())
}

fn signing_key(s: &Settings) -> Result<String> {
    s.signing_key.clone().ok_or_else(|| {
        CarryError::BadKey(
            "no signing key (set CARRY_TOOL_SIGNING_KEY or CARRY_TOOL_SIGNING_KEY_FILE)".into(),
        )
    })
}

// ---------------------------------------------------------------- JSON read-modify-write with If-Match

async fn read_json<T: serde::de::DeserializeOwned>(
    repo: &Repo,
    key: &str,
) -> Result<Option<(T, String)>> {
    match repo.store.get_with_etag(key).await? {
        None => Ok(None),
        Some((b, etag)) => {
            let v = serde_json::from_slice(&b)
                .map_err(|e| CarryError::Invalid(format!("{key}: {e}")))?;
            Ok(Some((v, etag)))
        }
    }
}

async fn write_json<T: serde::Serialize>(
    repo: &Repo,
    key: &str,
    v: &T,
    etag: Option<&str>,
) -> Result<bool> {
    let b =
        Bytes::from(serde_json::to_vec_pretty(v).map_err(|e| CarryError::Internal(anyhow!(e)))?);
    match etag {
        None => repo.store.put_if_absent(key, b).await,
        // No ETag means we cannot write conditionally; never fall back to a blind overwrite.
        Some("") => Err(CarryError::Conflict(format!(
            "{key}: store returned no ETag; refusing an unconditional overwrite"
        ))),
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
        tokio::time::sleep(std::time::Duration::from_millis(
            30 + rand::random::<u64>() % 100,
        ))
        .await;
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
        return Err(CarryError::Usage(format!(
            "{} already exists; refusing to overwrite",
            out.display()
        )));
    }
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)
            .map_err(|e| CarryError::Usage(format!("cannot create {}: {e}", p.display())))?;
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

fn http_client(timeout_secs: u64) -> Result<reqwest::Client> {
    let t = timeout_secs.max(1);
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(t))
        // Whole transfer (headers + body); generous because tool binaries can be large.
        .timeout(std::time::Duration::from_secs(t.saturating_mul(20)))
        .build()
        .map_err(|e| CarryError::Internal(anyhow!(e)))
}

async fn download(url: &str, timeout_secs: u64) -> Result<Vec<u8>> {
    use futures::StreamExt;
    let resp = http_client(timeout_secs)?
        .get(url)
        .send()
        .await
        .map_err(|e| CarryError::Unreachable(format!("{url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(CarryError::NotFound(format!(
            "{url}: HTTP {}",
            resp.status()
        )));
    }
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(c) = s.next().await {
        out.extend_from_slice(&c.map_err(|e| CarryError::Unreachable(format!("{url}: {e}")))?);
    }
    Ok(out)
}

pub async fn publish(
    s: &Settings,
    name: &str,
    version: &str,
    src: ToolSource,
    platform: &str,
) -> Result<Value> {
    valid_name("tool name", name)?;
    valid_name("version", version)?;
    if !platform.contains('/') {
        return Err(CarryError::Usage(format!(
            "platform must be os/arch, got '{platform}'"
        )));
    }
    let sk = signing_key(s)?;
    let me = crypto::ed25519_public(&sk)?;
    let repo = ctx::open_repo_for_write(s).await?;
    let (sha, size, blob) = match src {
        ToolSource::File(p) => {
            // Stream from disk: sha256 pass here, then the repo hashes and uploads in bounded chunks.
            let ue =
                |e: std::io::Error| CarryError::Usage(format!("cannot read {}: {e}", p.display()));
            let mut f = std::fs::File::open(&p).map_err(ue)?;
            let size = f.metadata().map_err(ue)?.len();
            let mut h = Sha256::new();
            std::io::copy(&mut f, &mut ShaSink(&mut h)).map_err(ue)?;
            let sha = hex::encode(h.finalize());
            let (blob, _) = repo.put_tool_blob_from(f, size).await?;
            (sha, size, blob)
        }
        ToolSource::Url { url, sha256 } => {
            let b = download(&url, s.timeout_secs).await?;
            if let Some(want) = sha256 {
                let got = sha256_hex(&b);
                if !got.eq_ignore_ascii_case(want.trim()) {
                    return Err(CarryError::Invalid(format!(
                        "sha256 mismatch for {url}: got {got}"
                    )));
                }
            }
            (
                sha256_hex(&b),
                b.len() as u64,
                repo.put_tool_blob(&b).await?,
            )
        }
    };
    let build = ToolBuild {
        sha256: sha.clone(),
        size,
        blob,
    };
    rmw::<SignedToolMeta, _>(&repo, &meta_key(name, version), |old| {
        let meta = match old {
            Some(old) => {
                if old.signer.trim() != me {
                    return Err(CarryError::Refused(format!(
                        "{name}@{version} was signed by a different key"
                    )));
                }
                verify_meta(&old, &me)?;
                let mut m = old.meta;
                m.platforms.insert(platform.to_string(), build.clone());
                m.seq += 1;
                m
            }
            None => ToolMeta {
                name: name.into(),
                version: version.into(),
                platforms: BTreeMap::from([(platform.to_string(), build.clone())]),
                yanked: false,
                published_at: chrono::Utc::now(),
                seq: 1,
            },
        };
        sign_meta(meta, &sk)
    })
    .await?;
    Ok(json!({"name": name, "version": version, "platform": platform, "sha256": sha}))
}

pub async fn promote(s: &Settings, name: &str, version: &str, channel: &str) -> Result<Value> {
    valid_name("channel", channel)?;
    let sk = signing_key(s)?;
    let me = crypto::ed25519_public(&sk)?;
    let repo = ctx::open_repo_for_write(s).await?;
    let m: SignedToolMeta = repo
        .get_tool_json(&meta_key(name, version))
        .await?
        .ok_or_else(|| CarryError::NotFound(format!("{name}@{version}")))?;
    if m.meta.yanked {
        return Err(CarryError::Refused(format!("{name}@{version} is yanked")));
    }
    verify_meta(&m, &me).map_err(|_| {
        CarryError::Refused(format!("{name}@{version} was signed by a different key"))
    })?;
    rmw::<Value, _>(&repo, &channels_key(name), |old| {
        let doc = match old {
            None => ChannelsDoc {
                name: name.into(),
                seq: 0,
                channels: Channels::new(),
            },
            Some(v) => match serde_json::from_value::<SignedChannels>(v.clone()) {
                Ok(sc) => {
                    verify_channels(&sc, &me).map_err(|_| {
                        CarryError::Refused(format!(
                            "{name} channels were signed by a different key"
                        ))
                    })?;
                    sc.payload
                }
                // Legacy unsigned map: migrate it into a signed doc.
                Err(_) => ChannelsDoc {
                    name: name.into(),
                    seq: 0,
                    channels: serde_json::from_value(v)
                        .map_err(|e| CarryError::Invalid(format!("channels.json: {e}")))?,
                },
            },
        };
        let mut doc = doc;
        doc.channels.insert(channel.into(), version.into());
        doc.seq += 1;
        serde_json::to_value(sign_channels(doc, &sk)?).map_err(|e| CarryError::Internal(anyhow!(e)))
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
        verify_meta(&old, &me).map_err(|_| {
            CarryError::Refused(format!("{name}@{version} was signed by a different key"))
        })?;
        let mut m = old.meta;
        m.yanked = true;
        m.seq += 1;
        sign_meta(m, &sk)
    })
    .await?;
    Ok(json!({"name": name, "version": version, "yanked": true}))
}

pub async fn ls(s: &Settings, name: Option<&str>) -> Result<Value> {
    let repo = ctx::open_repo(s).await?;
    let names = match name {
        Some(n) => vec![n.to_string()],
        None => repo
            .store
            .list_dirs("tools")
            .await?
            .into_iter()
            .filter(|n| n != "blobs")
            .collect(),
    };
    let mut tools = Vec::new();
    for n in names {
        let versions = repo.store.list_dirs(&format!("tools/{n}")).await?;
        let channels = repo
            .get_tool_json::<Value>(&channels_key(&n))
            .await?
            .map(|v| channels_for_display(&v))
            .unwrap_or_default();
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
        if repo
            .get_tool_json::<SignedToolMeta>(&meta_key(name, v))
            .await?
            .is_none()
        {
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
                return Err(CarryError::NotFound(format!(
                    "tool '{name}' in state '{state}'"
                )));
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

pub fn link_tools_to(
    root: &Path,
    apps: &BTreeMap<String, ToolRef>,
    exe: &Path,
) -> std::io::Result<()> {
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
pub fn resolve_version(
    name: &str,
    r: &ToolRef,
    channels: Option<&Channels>,
) -> std::result::Result<String, String> {
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

/// Check a fetched tool.json is for `name@version`, trusted, not yanked, and pick the
/// build for `platform`. A validly signed tool.json for another tool/version is refused.
pub fn select_build(
    m: &SignedToolMeta,
    pubkey: &str,
    name: &str,
    version: &str,
    platform: &str,
) -> std::result::Result<ToolBuild, String> {
    verify_meta(m, pubkey).map_err(|e| format!("refused: {e}"))?;
    if m.meta.name != name || m.meta.version != version {
        return Err(format!(
            "refused: metadata is for {}@{}, not {name}@{version}",
            m.meta.name, m.meta.version
        ));
    }
    if m.meta.yanked {
        return Err(format!("{}@{} is yanked", m.meta.name, m.meta.version));
    }
    m.meta.platforms.get(platform).cloned().ok_or_else(|| {
        format!(
            "{}@{} has no build for {platform}",
            m.meta.name, m.meta.version
        )
    })
}

fn cache_dir(root: &Path) -> PathBuf {
    root.join("cache/tools")
}

/// Entry point when carry runs under a tool's name. Returns an exit code only on failure.
pub fn launch_from_argv0(name: &str) -> i32 {
    let root = std::env::var("CARRY_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/carry"));
    let outcome = prepare(name);
    // Keep `carry status` truthful for on-demand launches, not just prefetch.
    match &outcome {
        Ok(_) => {
            let ready = read_status(&root)
                .get(name)
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str())
                == Some("ready");
            if !ready {
                record_status(&root, &[(name.to_string(), Ok(()))]);
            }
        }
        Err(e) => record_status(&root, &[(name.to_string(), Err(e.clone()))]),
    }
    match outcome {
        Ok(bin) => {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&bin)
                .arg0(name)
                .args(std::env::args_os().skip(1))
                .exec();
            eprintln!("carry: {name}: exec {}: {err}", bin.display());
            127
        }
        Err(e) => {
            eprintln!("carry: {name}: {e}");
            127
        }
    }
}

fn launcher_pubkey(s: Option<&Settings>) -> std::result::Result<String, String> {
    s.and_then(|s| s.tool_pubkey.clone())
        .or_else(|| std::env::var("CARRY_TOOL_PUBKEY").ok())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "refused: CARRY_TOOL_PUBKEY is not set".to_string())
}

fn prepare(name: &str) -> std::result::Result<PathBuf, String> {
    let root = std::env::var("CARRY_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/carry"));
    let apps = read_apps(&root)?;
    let r = apps
        .get(name)
        .ok_or_else(|| "not listed in this state's apps".to_string())?;
    let pubkey = launcher_pubkey(None)?;
    let platform = current_platform();
    let meta_cache = cache_dir(&root).join("meta").join(name);

    // Fast path: pinned version with verified metadata and binary already cached.
    if let Some(v) = &r.version {
        let mf = meta_cache.join(format!("{v}.json"));
        if let Some(bin) = cached_binary(&root, &mf, &pubkey, name, v, &platform) {
            // Best effort: honour a yank if the store answers within 1 s; offline -> run cached.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt {
                let fetch_meta = async {
                    let s = Settings::resolve(&crate::cli::GlobalArgs::default())?;
                    let repo = ctx::open_repo(&s).await?;
                    repo.get_tool_json::<SignedToolMeta>(&meta_key(name, v))
                        .await
                };
                let fresh = rt.block_on(yank_check(fetch_meta, &pubkey, YANK_CHECK_BUDGET));
                rt.shutdown_background();
                if let Some(m) = fresh.filter(|m| m.meta.name == *name && m.meta.version == *v) {
                    // Records a yank (sticky) and refuses replays of older metadata.
                    check_meta_hwm(&root, &m.meta)?;
                    if m.meta.yanked {
                        if let Ok(b) = serde_json::to_vec(&m) {
                            let _ = std::fs::write(&mf, b);
                        }
                        return Err(format!("refused: {name}@{v} is yanked"));
                    }
                }
            }
            return Ok(bin);
        }
    }

    let s = Settings::resolve(&crate::cli::GlobalArgs::default()).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let out = rt.block_on(fetch_bounded(&s, name, r, &root, &pubkey, &platform));
    rt.shutdown_background();
    out
}

const YANK_CHECK_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

/// Fetch fresh metadata within `budget`. Returns it only if it arrived in time and is
/// signed by `pubkey`; any error, timeout or bad signature yields None (keep the cache).
async fn yank_check<F>(
    fetch: F,
    pubkey: &str,
    budget: std::time::Duration,
) -> Option<SignedToolMeta>
where
    F: std::future::Future<Output = Result<Option<SignedToolMeta>>>,
{
    let m = tokio::time::timeout(budget, fetch).await.ok()?.ok()??;
    verify_meta(&m, pubkey).ok()?;
    Some(m)
}

fn cached_binary(
    root: &Path,
    meta_file: &Path,
    pubkey: &str,
    name: &str,
    version: &str,
    platform: &str,
) -> Option<PathBuf> {
    if locally_yanked(root, name, version) {
        return None;
    }
    let b = std::fs::read(meta_file).ok()?;
    let m: SignedToolMeta = serde_json::from_slice(&b).ok()?;
    let build = select_build(&m, pubkey, name, version, platform).ok()?;
    let bin = cache_dir(root).join(&build.sha256);
    bin.is_file().then_some(bin)
}

/// `fetch` bounded by CARRY_TIMEOUT (settings.timeout_secs) end to end, lock wait included.
async fn fetch_bounded(
    s: &Settings,
    name: &str,
    r: &ToolRef,
    root: &Path,
    pubkey: &str,
    platform: &str,
) -> std::result::Result<PathBuf, String> {
    let secs = s.timeout_secs.max(1);
    let work = async {
        let repo = ctx::open_repo(s).await.map_err(|e| e.to_string())?;
        fetch_with_repo(&repo, name, r, root, pubkey, platform).await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(secs), work).await {
        Ok(x) => x,
        Err(_) => Err(format!(
            "timed out after {secs}s fetching from the tool store (CARRY_TIMEOUT)"
        )),
    }
}

/// Take the per-tool flock, polling a non-blocking try-lock so the caller's timeout can cancel it.
async fn lock_polling(
    path: &Path,
) -> std::result::Result<nix::fcntl::Flock<std::fs::File>, String> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("lock: {e}"))?;
    loop {
        match nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(l) => return Ok(l),
            Err((back, nix::errno::Errno::EWOULDBLOCK)) => {
                f = back;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err((_, e)) => return Err(format!("lock: {e}")),
        }
    }
}

async fn fetch_with_repo(
    repo: &Repo,
    name: &str,
    r: &ToolRef,
    root: &Path,
    pubkey: &str,
    platform: &str,
) -> std::result::Result<PathBuf, String> {
    let meta_cache = cache_dir(root).join("meta").join(name);
    let version = if r.version.is_some() {
        resolve_version(name, r, None)?
    } else {
        let raw: Option<Value> = repo
            .get_tool_json(&channels_key(name))
            .await
            .map_err(|e| e.to_string())?;
        let ch = match raw {
            None => None,
            Some(v) => {
                let doc = check_channels(&v, name, pubkey)?;
                check_channels_hwm(root, &doc)?;
                Some(doc.channels)
            }
        };
        resolve_version(name, r, ch.as_ref())?
    };
    let m: SignedToolMeta = repo
        .get_tool_json(&meta_key(name, &version))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{name}@{version} not found in the tool store"))?;
    // Identity + signature first, then anti-replay bookkeeping, then yank/platform.
    verify_meta(&m, pubkey).map_err(|e| format!("refused: {e}"))?;
    if m.meta.name != name || m.meta.version != version {
        return Err(format!(
            "refused: metadata is for {}@{}, not {name}@{version}",
            m.meta.name, m.meta.version
        ));
    }
    check_meta_hwm(root, &m.meta)?;
    let build = select_build(&m, pubkey, name, &version, platform)?;
    let dir = cache_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bin = dir.join(&build.sha256);

    // One downloader per tool; others wait, then find the binary present.
    let _lock = lock_polling(&dir.join(format!(".lock-{name}"))).await?;

    if !bin.is_file() {
        let tmp = dir.join(format!(".{}.tmp{}", build.sha256, std::process::id()));
        let got = download_blob(repo, &build.blob, &tmp).await;
        let got = match got {
            Ok(g) => g,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if got != build.sha256 {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "refused: sha256 mismatch (want {}, got {got})",
                build.sha256
            ));
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod: {e}"))?;
        std::fs::rename(&tmp, &bin).map_err(|e| format!("cache rename: {e}"))?;
    }
    // Remember verified metadata so later pinned launches need no network.
    if std::fs::create_dir_all(&meta_cache).is_ok() {
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

// ---------------------------------------------------------------- prefetch

/// Download + verify tools into `<root>/cache/tools` without exec'ing them, using the
/// launcher's fetch path (signature, identity, anti-replay, sha256, timeout). Selects
/// tools with `required` (when `only_required`) or `prefetch || required` otherwise.
/// Records each result in `<root>/cache/tools/status.json` as
/// `{"<name>": {"status": "ready"|"failed", "reason"?: "...", "at": "..."}}`.
pub async fn prefetch_tools(
    settings: &Settings,
    root: &Path,
    apps: &BTreeMap<String, ToolRef>,
    only_required: bool,
) -> Vec<(String, std::result::Result<(), String>)> {
    let picked: Vec<(&String, &ToolRef)> = apps
        .iter()
        .filter(|(_, r)| {
            if only_required {
                r.required
            } else {
                r.prefetch || r.required
            }
        })
        .collect();
    if picked.is_empty() {
        return Vec::new();
    }
    let platform = current_platform();
    let mut out = Vec::new();
    match launcher_pubkey(Some(settings)) {
        Err(e) => {
            for (n, _) in picked {
                out.push((n.clone(), Err(e.clone())));
            }
        }
        Ok(pk) => {
            for (n, r) in picked {
                let res = if valid_name("tool name", n).is_err() {
                    Err(format!("bad tool name '{n}'"))
                } else {
                    fetch_bounded(settings, n, r, root, &pk, &platform)
                        .await
                        .map(|_| ())
                };
                out.push((n.clone(), res));
            }
        }
    }
    record_status(root, &out);
    out
}

/// Prefetch with an already-open repo (tests).
async fn prefetch_with_repo(
    repo: &Repo,
    root: &Path,
    apps: &BTreeMap<String, ToolRef>,
    pubkey: &str,
) -> Vec<(String, std::result::Result<(), String>)> {
    let platform = current_platform();
    let mut out = Vec::new();
    for (n, r) in apps.iter().filter(|(_, r)| r.prefetch || r.required) {
        let res = fetch_with_repo(repo, n, r, root, pubkey, &platform)
            .await
            .map(|_| ());
        out.push((n.clone(), res));
    }
    record_status(root, &out);
    out
}

pub fn status_path(root: &Path) -> PathBuf {
    cache_dir(root).join("status.json")
}

/// Per-tool status recorded by the last prefetch: name -> {"status", "reason"?}.
pub fn read_status(root: &Path) -> BTreeMap<String, Value> {
    std::fs::read(status_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn record_status(root: &Path, results: &[(String, std::result::Result<(), String>)]) {
    let mut st = read_status(root);
    let at = chrono::Utc::now().to_rfc3339();
    for (n, r) in results {
        let v = match r {
            Ok(()) => json!({"status": "ready", "at": at}),
            Err(e) => json!({"status": "failed", "reason": e, "at": at}),
        };
        st.insert(n.clone(), v);
    }
    let p = status_path(root);
    let Some(dir) = p.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".status.tmp{}", std::process::id()));
    if let Ok(b) = serde_json::to_vec_pretty(&st) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
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
async fn download_blob(
    repo: &Repo,
    blob: &str,
    dest: &Path,
) -> std::result::Result<String, String> {
    use sha2::Digest;
    let f = std::fs::File::create(dest).map_err(|e| format!("cache write: {e}"))?;
    let mut w = Sha256Writer {
        inner: std::io::BufWriter::with_capacity(256 << 10, f),
        h: sha2::Sha256::new(),
    };
    repo.get_tool_blob_to(blob, &mut w)
        .await
        .map_err(|e| e.to_string())?;
    let f = w
        .inner
        .into_inner()
        .map_err(|e| format!("cache write: {}", e.error()))?;
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
                ToolBuild {
                    sha256: "ab".into(),
                    size: 2,
                    blob: "b1".into(),
                },
            )]),
            yanked,
            published_at: chrono::Utc::now(),
            seq: 1,
        }
    }

    #[test]
    fn sign_and_verify() {
        let (sk, pk) = crypto::ed25519_keygen();
        let s = sign_meta(meta(false), &sk).unwrap();
        assert_eq!(s.signer, pk);
        verify_meta(&s, &pk).unwrap();
        // Round-trip through JSON still verifies.
        let back: SignedToolMeta =
            serde_json::from_str(&serde_json::to_string_pretty(&s).unwrap()).unwrap();
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
        assert!(select_build(&s, &pk, "jq", "1.7", "linux/amd64")
            .unwrap_err()
            .starts_with("refused"));
        let (sk2, _) = crypto::ed25519_keygen();
        let foreign = sign_meta(meta(false), &sk2).unwrap();
        assert!(select_build(&foreign, &pk, "jq", "1.7", "linux/amd64")
            .unwrap_err()
            .starts_with("refused"));
    }

    #[test]
    fn yank_and_channels() {
        let (sk, pk) = crypto::ed25519_keygen();
        let ok = sign_meta(meta(false), &sk).unwrap();
        assert_eq!(
            select_build(&ok, &pk, "jq", "1.7", "linux/amd64")
                .unwrap()
                .sha256,
            "ab"
        );
        assert!(select_build(&ok, &pk, "jq", "1.7", "linux/arm64")
            .unwrap_err()
            .contains("no build"));
        let y = sign_meta(meta(true), &sk).unwrap();
        assert!(select_build(&y, &pk, "jq", "1.7", "linux/amd64")
            .unwrap_err()
            .contains("yanked"));

        let ch = Channels::from([("stable".into(), "1.7".into())]);
        let pinned = ToolRef {
            version: Some("1.6".into()),
            ..Default::default()
        };
        assert_eq!(resolve_version("jq", &pinned, Some(&ch)).unwrap(), "1.6");
        let dflt = ToolRef::default();
        assert_eq!(resolve_version("jq", &dflt, Some(&ch)).unwrap(), "1.7");
        let beta = ToolRef {
            channel: Some("beta".into()),
            ..Default::default()
        };
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
        assert!(yank_check(
            async { Err(CarryError::Unreachable("down".into())) },
            &pk,
            b
        )
        .await
        .is_none());
        // Forged metadata cannot yank (or un-yank) anything.
        let (_, other) = crypto::ed25519_keygen();
        assert!(yank_check(async { Ok(Some(y.clone())) }, &other, b)
            .await
            .is_none());
    }

    #[test]
    fn cache_hit_needs_no_network() {
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        let plat = current_platform();
        let body = b"#!/bin/sh\necho hi\n";
        let sha = sha256_hex(body);
        let mut m = meta(false);
        m.platforms = BTreeMap::from([(
            plat.clone(),
            ToolBuild {
                sha256: sha.clone(),
                size: 3,
                blob: "x".into(),
            },
        )]);
        let s = sign_meta(m, &sk).unwrap();
        let mf = d.path().join("cache/tools/meta/jq/1.7.json");
        std::fs::create_dir_all(mf.parent().unwrap()).unwrap();
        std::fs::write(&mf, serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(cached_binary(d.path(), &mf, &pk, "jq", "1.7", &plat).is_none());
        std::fs::write(d.path().join("cache/tools").join(&sha), body).unwrap();
        assert_eq!(
            cached_binary(d.path(), &mf, &pk, "jq", "1.7", &plat).unwrap(),
            d.path().join("cache/tools").join(&sha)
        );
        let (_, other) = crypto::ed25519_keygen();
        assert!(cached_binary(d.path(), &mf, &other, "jq", "1.7", &plat).is_none());
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
        assert_eq!(
            crypto::ed25519_public(&secret).unwrap(),
            v["public_key"].as_str().unwrap()
        );
        assert!(!v.to_string().contains(secret.trim()));
        assert!(keygen_to(&p).is_err());
    }

    #[test]
    fn apps_from_manifest() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("current")).unwrap();
        std::fs::write(
            d.path().join("current/manifest.json"),
            r#"{"apps":{"jq":{"version":"1.7"}}}"#,
        )
        .unwrap();
        let a = read_apps(d.path()).unwrap();
        assert_eq!(a["jq"].version.as_deref(), Some("1.7"));
    }

    async fn mem_repo() -> Repo {
        let st = crate::store::Store::from_object_store(
            std::sync::Arc::new(object_store::memory::InMemory::new()),
            "r",
        );
        Repo::open_or_init(st, None, true).await.unwrap()
    }

    async fn put_json<T: serde::Serialize>(repo: &Repo, key: &str, v: &T) {
        repo.store
            .put(key, Bytes::from(serde_json::to_vec(v).unwrap()))
            .await
            .unwrap();
    }

    /// Publish `body` as name@version for this platform into an in-memory repo.
    async fn publish_mem(
        repo: &Repo,
        sk: &str,
        name: &str,
        version: &str,
        body: &[u8],
        seq: u64,
        yanked: bool,
    ) {
        let blob = repo.put_tool_blob(body).await.unwrap();
        let m = ToolMeta {
            name: name.into(),
            version: version.into(),
            platforms: BTreeMap::from([(
                current_platform(),
                ToolBuild {
                    sha256: sha256_hex(body),
                    size: body.len() as u64,
                    blob,
                },
            )]),
            yanked,
            published_at: chrono::Utc::now(),
            seq,
        };
        put_json(repo, &meta_key(name, version), &sign_meta(m, sk).unwrap()).await;
    }

    #[test]
    fn identity_mismatch_refused() {
        let (sk, pk) = crypto::ed25519_keygen();
        let s = sign_meta(meta(false), &sk).unwrap(); // jq@1.7, validly signed
        assert!(select_build(&s, &pk, "yq", "1.7", "linux/amd64")
            .unwrap_err()
            .contains("not yq@1.7"));
        assert!(select_build(&s, &pk, "jq", "1.6", "linux/amd64")
            .unwrap_err()
            .contains("not jq@1.6"));
    }

    #[test]
    fn legacy_meta_without_seq_still_verifies() {
        let (sk, pk) = crypto::ed25519_keygen();
        let mut m = meta(false);
        m.seq = 0;
        let s = sign_meta(m, &sk).unwrap();
        let txt = serde_json::to_string(&s).unwrap();
        assert!(!txt.contains("\"seq\""));
        let back: SignedToolMeta = serde_json::from_str(&txt).unwrap();
        verify_meta(&back, &pk).unwrap();
    }

    #[test]
    fn meta_replay_and_sticky_yank() {
        let d = tempfile::tempdir().unwrap();
        let mut m = meta(false);
        m.seq = 3;
        check_meta_hwm(d.path(), &m).unwrap();
        m.seq = 2; // downgrade/replay of older metadata
        assert!(check_meta_hwm(d.path(), &m).unwrap_err().contains("replay"));
        m.seq = 4;
        m.yanked = true;
        check_meta_hwm(d.path(), &m).unwrap();
        assert!(locally_yanked(d.path(), "jq", "1.7"));
        // Un-yank with a higher seq is still refused: the yank is sticky locally.
        m.seq = 9;
        m.yanked = false;
        assert!(check_meta_hwm(d.path(), &m).unwrap_err().contains("yanked"));
        // Another version is unaffected.
        m.version = "1.8".into();
        check_meta_hwm(d.path(), &m).unwrap();
    }

    #[test]
    fn channels_must_be_signed_for_this_tool_and_fresh() {
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        let doc = |name: &str, seq| ChannelsDoc {
            name: name.into(),
            seq,
            channels: Channels::from([("stable".into(), "1.7".into())]),
        };
        let good = serde_json::to_value(sign_channels(doc("jq", 2), &sk).unwrap()).unwrap();
        assert_eq!(
            check_channels(&good, "jq", &pk).unwrap().channels["stable"],
            "1.7"
        );
        // Legacy unsigned map.
        let unsigned = json!({"stable": "1.7"});
        assert!(check_channels(&unsigned, "jq", &pk)
            .unwrap_err()
            .contains("unsigned"));
        // Foreign signer.
        let (sk2, _) = crypto::ed25519_keygen();
        let foreign = serde_json::to_value(sign_channels(doc("jq", 2), &sk2).unwrap()).unwrap();
        assert!(check_channels(&foreign, "jq", &pk)
            .unwrap_err()
            .starts_with("refused"));
        // Signed for another tool.
        let other = serde_json::to_value(sign_channels(doc("yq", 2), &sk).unwrap()).unwrap();
        assert!(check_channels(&other, "jq", &pk)
            .unwrap_err()
            .contains("not 'jq'"));
        // Tampered payload.
        let mut t = good.clone();
        t["payload"]["channels"]["stable"] = json!("0.1");
        assert!(check_channels(&t, "jq", &pk).is_err());
        // Replay of older channels.
        check_channels_hwm(d.path(), &doc("jq", 2)).unwrap();
        assert!(check_channels_hwm(d.path(), &doc("jq", 1))
            .unwrap_err()
            .contains("replay"));
        check_channels_hwm(d.path(), &doc("jq", 2)).unwrap();
    }

    #[tokio::test]
    async fn empty_etag_is_a_conflict_not_a_blind_put() {
        let repo = mem_repo().await;
        put_json(&repo, "tools/jq/channels.json", &json!({"a": 1})).await;
        let e = write_json(&repo, "tools/jq/channels.json", &json!({"b": 2}), Some(""))
            .await
            .unwrap_err();
        assert!(matches!(e, CarryError::Conflict(_)));
        let (v, _) = read_json::<Value>(&repo, "tools/jq/channels.json")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v, json!({"a": 1}));
    }

    #[tokio::test]
    async fn lock_wait_is_cancellable() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".lock-jq");
        let held = lock_polling(&p).await.unwrap();
        let t = std::time::Instant::now();
        let r = tokio::time::timeout(std::time::Duration::from_millis(300), lock_polling(&p)).await;
        assert!(r.is_err(), "second locker must still be waiting");
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        drop(held);
        lock_polling(&p).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_refuses_swapped_meta_and_unsigned_channels() {
        let repo = mem_repo().await;
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        let plat = current_platform();
        publish_mem(
            &repo,
            &sk,
            "evil",
            "1.0",
            b"#!/bin/sh\necho evil\n",
            1,
            false,
        )
        .await;
        // Copy evil's signed tool.json to jq@1.0.
        let evil: SignedToolMeta = repo
            .get_tool_json(&meta_key("evil", "1.0"))
            .await
            .unwrap()
            .unwrap();
        put_json(&repo, &meta_key("jq", "1.0"), &evil).await;
        let pinned = ToolRef {
            version: Some("1.0".into()),
            ..Default::default()
        };
        let e = fetch_with_repo(&repo, "jq", &pinned, d.path(), &pk, &plat)
            .await
            .unwrap_err();
        assert!(e.contains("not jq@1.0"), "{e}");
        // Unsigned channels.json is refused for channel lookups.
        put_json(&repo, &channels_key("jq"), &json!({"stable": "1.0"})).await;
        let e = fetch_with_repo(&repo, "jq", &ToolRef::default(), d.path(), &pk, &plat)
            .await
            .unwrap_err();
        assert!(e.contains("unsigned"), "{e}");
    }

    #[tokio::test]
    async fn fetch_refuses_replayed_unyank() {
        let repo = mem_repo().await;
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        let plat = current_platform();
        let pinned = ToolRef {
            version: Some("1.0".into()),
            ..Default::default()
        };
        publish_mem(&repo, &sk, "jq", "1.0", b"v1", 1, false).await;
        let old: SignedToolMeta = repo
            .get_tool_json(&meta_key("jq", "1.0"))
            .await
            .unwrap()
            .unwrap();
        fetch_with_repo(&repo, "jq", &pinned, d.path(), &pk, &plat)
            .await
            .unwrap();
        publish_mem(&repo, &sk, "jq", "1.0", b"v1", 2, true).await;
        assert!(fetch_with_repo(&repo, "jq", &pinned, d.path(), &pk, &plat)
            .await
            .unwrap_err()
            .contains("yanked"));
        // Attacker restores the old, validly signed, un-yanked tool.json.
        put_json(&repo, &meta_key("jq", "1.0"), &old).await;
        let e = fetch_with_repo(&repo, "jq", &pinned, d.path(), &pk, &plat)
            .await
            .unwrap_err();
        assert!(e.contains("replay") || e.contains("yanked"), "{e}");
    }

    #[tokio::test]
    async fn prefetch_records_status() {
        let repo = mem_repo().await;
        let d = tempfile::tempdir().unwrap();
        let (sk, pk) = crypto::ed25519_keygen();
        publish_mem(&repo, &sk, "jq", "1.7", b"jq-binary", 1, false).await;
        let chans = ChannelsDoc {
            name: "jq".into(),
            seq: 1,
            channels: Channels::from([("stable".into(), "1.7".into())]),
        };
        put_json(
            &repo,
            &channels_key("jq"),
            &sign_channels(chans, &sk).unwrap(),
        )
        .await;
        let apps = BTreeMap::from([
            (
                "jq".to_string(),
                ToolRef {
                    prefetch: true,
                    ..Default::default()
                },
            ),
            (
                "yq".to_string(),
                ToolRef {
                    version: Some("4".into()),
                    required: true,
                    ..Default::default()
                },
            ),
            ("lazy".to_string(), ToolRef::default()),
        ]);
        let res = prefetch_with_repo(&repo, d.path(), &apps, &pk).await;
        assert_eq!(res.len(), 2, "tools without prefetch/required are skipped");
        let st = read_status(d.path());
        assert_eq!(st["jq"]["status"], "ready");
        assert_eq!(st["yq"]["status"], "failed");
        assert!(st["yq"]["reason"].as_str().unwrap().contains("not found"));
        assert!(!st.contains_key("lazy"));
        assert!(d
            .path()
            .join("cache/tools")
            .join(sha256_hex(b"jq-binary"))
            .is_file());
    }
}
