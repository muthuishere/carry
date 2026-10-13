//! Local restore: HEAD -> atomic generation under `<root>/gens/<seq>`, `current`
//! symlink flip, `status.json`, and materialising manifest paths as symlinks.
//! Fail-open (ADR 0005): every failure degrades to the last good generation.
use crate::ctx;
use crate::error::{CarryError, Result};
use crate::model::{Entry, EntryKind, Manifest};
use crate::repo::Repo;
use crate::settings::Settings;
use anyhow::anyhow;
use base64::Engine;
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::panic::AssertUnwindSafe;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Blobs in flight at once: bounds memory to ~2 blobs regardless of tree size.
const BLOB_CONCURRENCY: usize = 2;
const KEEP_GENERATIONS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Ok,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub mode: Mode,
    /// Generation now in `current` (manifest seq), None if empty.
    pub version: Option<u64>,
    pub ms: u64,
    pub error: Option<String>,
}

/// State name used by `run`/`status`: CARRY_STATE, else HOSTNAME, else "default".
pub fn state_name(s: &Settings) -> String {
    s.state
        .clone()
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "default".into())
}

/// Restore `state` into `root` within `budget`. Never fails, never panics out.
pub async fn restore(s: &Settings, state: &str, root: &Path, budget: Duration) -> RestoreReport {
    let s = s.clone();
    restore_with(async move { ctx::open_repo(&s).await }, state, root, budget).await
}

/// Same as `restore` with an injectable repo opener (tests use InMemory).
pub async fn restore_with<F>(open: F, state: &str, root: &Path, budget: Duration) -> RestoreReport
where
    F: std::future::Future<Output = Result<Repo>>,
{
    let start = Instant::now();
    let work = async {
        let repo = open.await?;
        sync_generation(&repo, state, root).await
    };
    let outcome = AssertUnwindSafe(tokio::time::timeout(budget, work))
        .catch_unwind()
        .await;
    let result: Result<Option<u64>> = match outcome {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(CarryError::Unreachable(format!(
            "restore exceeded budget of {} ms",
            budget.as_millis()
        ))),
        Err(_) => Err(CarryError::Internal(anyhow!("restore panicked"))),
    };
    let ms = start.elapsed().as_millis() as u64;
    let report = match result {
        Ok(version) => RestoreReport {
            mode: Mode::Ok,
            version,
            ms,
            error: None,
        },
        Err(e) => {
            // Keep the last good generation; create an empty one if there is none.
            let version = match current_generation(root) {
                Some(v) => Some(v),
                None => {
                    if let Err(e2) = ensure_empty_generation(root) {
                        eprintln!("carry: cannot create empty generation: {e2}");
                    }
                    None
                }
            };
            RestoreReport {
                mode: Mode::Degraded,
                version: version.filter(|v| *v > 0),
                ms,
                error: Some(e.to_string()),
            }
        }
    };
    if let Err(e) = write_status(root, state, &report) {
        eprintln!("carry: cannot write status.json: {e}");
    }
    cleanup(root);
    report
}

/// Seq the `current` symlink points at, if any.
pub fn current_generation(root: &Path) -> Option<u64> {
    let t = std::fs::read_link(root.join("current")).ok()?;
    t.file_name()?.to_str()?.parse().ok()
}

/// Plaintext manifest copy of the current generation.
pub fn current_manifest(root: &Path) -> Option<Manifest> {
    let b = std::fs::read(root.join("current").join("manifest.json")).ok()?;
    serde_json::from_slice(&b).ok()
}

/// Returns the seq now in `current` (None when the state has no versions: empty generation).
async fn sync_generation(repo: &Repo, state: &str, root: &Path) -> Result<Option<u64>> {
    let mark = read_hwm(root).get(state).copied().unwrap_or(0);
    let m = match ctx::current(repo, state).await? {
        Some((_, _, m)) => m,
        None if mark > 0 => {
            return Err(CarryError::Invalid(format!(
                "HEAD of '{state}' vanished after version {mark} was seen"
            )));
        }
        None => {
            ensure_empty_generation(root).map_err(io)?;
            return Ok(None);
        }
    };
    if m.seq < mark {
        return Err(CarryError::Invalid(format!(
            "HEAD of '{state}' went back to version {} after version {mark} was seen; keeping current generation",
            m.seq
        )));
    }
    if current_generation(root) == Some(m.seq) && root.join("current/manifest.json").exists() {
        return Ok(Some(m.seq));
    }
    write_generation(repo, &m, root).await?;
    if let Err(e) = write_hwm(root, state, m.seq) {
        eprintln!("carry: warning: cannot record high-water mark: {e}");
    }
    Ok(Some(m.seq))
}

const HWM_FILE: &str = "state-hwm.json";

/// Highest HEAD seq ever restored per state (rollback / replay protection).
fn read_hwm(root: &Path) -> std::collections::BTreeMap<String, u64> {
    std::fs::read(root.join(HWM_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_hwm(root: &Path, state: &str, seq: u64) -> std::io::Result<()> {
    let mut h = read_hwm(root);
    let cur = h.entry(state.to_string()).or_insert(0);
    if seq <= *cur {
        return Ok(());
    }
    *cur = seq;
    let tmp = root.join(".state-hwm.json.tmp");
    write_file(
        &tmp,
        &serde_json::to_vec(&h).map_err(std::io::Error::other)?,
        0o600,
    )?;
    std::fs::rename(tmp, root.join(HWM_FILE))
}

/// Entries that conflict with another entry (ancestor/descendant). Such a manifest predates
/// the write-time check; these entries are skipped rather than failing the whole restore.
fn conflicting(m: &Manifest) -> std::collections::BTreeSet<String> {
    m.entries
        .keys()
        .filter(|p| ctx::path_conflict(&m.entries, p).is_some())
        .cloned()
        .collect()
}

/// Create `rel`'s directories under `base`, refusing to traverse anything that is not a
/// real directory (a symlink planted in the tree can never redirect a write outside it).
fn mkdirs_inside(base: &Path, rel: &Path) -> std::io::Result<()> {
    let mut cur = base.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is not a real directory",
                    cur.display()
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&cur)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

async fn write_generation(repo: &Repo, m: &Manifest, root: &Path) -> Result<()> {
    let gens = root.join("gens");
    let tmp = gens.join(format!("{}.tmp", m.seq));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(io)?;
    }
    let files = tmp.join("files");
    std::fs::create_dir_all(&files).map_err(io)?;

    let skip = conflicting(m);
    for p in &skip {
        eprintln!("carry: warning: skipping {p}: conflicts with another entry (file and directory at once)");
    }
    let mut jobs = Vec::new();
    for (path, e) in m.entries.iter().filter(|(p, _)| !skip.contains(*p)) {
        let r = rel(path)?;
        if let Some(p) = r.parent() {
            if let Err(err) = mkdirs_inside(&files, p) {
                eprintln!("carry: warning: skipping {path}: {err}");
                continue;
            }
        }
        let dest = files.join(r);
        match e.kind {
            EntryKind::Symlink => {
                let t = e.target.as_deref().ok_or_else(|| {
                    CarryError::Invalid(format!("{path}: symlink without target"))
                })?;
                std::os::unix::fs::symlink(t, &dest).map_err(io)?;
            }
            EntryKind::File => jobs.push((path.clone(), e.clone(), dest)),
        }
    }
    let mut stream = futures::stream::iter(jobs)
        .map(|(path, e, dest)| async move { write_entry(repo, &path, &e, &dest).await })
        .buffer_unordered(BLOB_CONCURRENCY);
    while let Some(r) = stream.next().await {
        r?;
    }
    drop(stream);

    let mf = tmp.join("manifest.json");
    write_file(
        &mf,
        &serde_json::to_vec(m).map_err(|e| CarryError::Internal(e.into()))?,
        0o600,
    )
    .map_err(io)?;
    fsync_dir(&tmp);

    let final_dir = gens.join(m.seq.to_string());
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir).map_err(io)?;
    }
    std::fs::rename(&tmp, &final_dir).map_err(io)?;
    fsync_dir(&gens);
    switch_current(root, m.seq).map_err(io)
}

/// Write one file entry to `dest`, streaming blobs with bounded memory.
async fn write_entry(repo: &Repo, path: &str, e: &Entry, dest: &Path) -> Result<()> {
    let n = if let Some(b64) = &e.inline {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|err| CarryError::Invalid(format!("{path}: bad inline content: {err}")))?;
        write_file(dest, &bytes, e.mode).map_err(io)?;
        bytes.len() as u64
    } else {
        match &e.blob {
            Some(id) => {
                let f = open_dest(dest, e.mode).map_err(io)?;
                let mut w = std::io::BufWriter::with_capacity(256 << 10, f);
                let n = repo.get_blob_to(id, &mut w).await?;
                let f = w.into_inner().map_err(|e| io(e.into_error()))?;
                f.sync_all().map_err(io)?;
                n
            }
            None if e.size == 0 => {
                write_file(dest, &[], e.mode).map_err(io)?;
                0
            }
            None => return Err(CarryError::Invalid(format!("{path}: entry has no content"))),
        }
    };
    if n != e.size {
        return Err(CarryError::Invalid(format!(
            "{path}: size {n} != manifest {}",
            e.size
        )));
    }
    Ok(())
}

fn open_dest(dest: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NOFOLLOW: never write through a symlink sitting at the destination.
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(dest)?;
    f.set_permissions(std::fs::Permissions::from_mode(mode & 0o7777))?;
    Ok(f)
}

fn write_file(dest: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let mut f = open_dest(dest, mode)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn fsync_dir(p: &Path) {
    if let Ok(d) = std::fs::File::open(p) {
        let _ = d.sync_all();
    }
}

/// `/etc/a` -> `etc/a`, rejecting `..` and non-absolute keys.
fn rel(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(CarryError::Invalid(format!(
            "entry path not absolute: {path}"
        )));
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => out.push(n),
            _ => return Err(CarryError::Invalid(format!("bad entry path: {path}"))),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(CarryError::Invalid(format!("bad entry path: {path}")));
    }
    Ok(out)
}

fn io(e: std::io::Error) -> CarryError {
    CarryError::Internal(e.into())
}

/// Atomically point `<root>/current` at `gens/<seq>` (tmp symlink + rename).
fn switch_current(root: &Path, seq: u64) -> std::io::Result<()> {
    let tmp = root.join(format!(".current.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("gens").join(seq.to_string()), &tmp)?;
    std::fs::rename(&tmp, root.join("current"))?;
    fsync_dir(root);
    Ok(())
}

fn ensure_empty_generation(root: &Path) -> std::io::Result<()> {
    let g = root.join("gens/0");
    std::fs::create_dir_all(g.join("files"))?;
    let m = ctx::empty_manifest(ctx::make_op("init", vec![]));
    write_file(
        &g.join("manifest.json"),
        &serde_json::to_vec(&m).map_err(std::io::Error::other)?,
        0o600,
    )?;
    switch_current(root, 0)
}

fn write_status(root: &Path, state: &str, r: &RestoreReport) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let mode = match r.mode {
        Mode::Ok => "ok",
        Mode::Degraded => "degraded",
    };
    let v = json!({
        "state": state, "mode": mode, "version": r.version, "root": root.display().to_string(),
        "last_restore_ms": r.ms, "error": r.error,
    });
    let tmp = root.join(".status.json.tmp");
    write_file(
        &tmp,
        &serde_json::to_vec(&v).map_err(std::io::Error::other)?,
        0o644,
    )?;
    std::fs::rename(tmp, root.join("status.json"))
}

/// Drop stale `*.tmp` dirs and all but the newest generations (never `current`).
fn cleanup(root: &Path) {
    let gens = root.join("gens");
    let cur = current_generation(root);
    let Ok(rd) = std::fs::read_dir(&gens) else {
        return;
    };
    let mut seqs = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".tmp") {
            let _ = std::fs::remove_dir_all(e.path());
        } else if let Ok(n) = name.parse::<u64>() {
            seqs.push(n);
        }
    }
    seqs.sort_unstable_by(|a, b| b.cmp(a));
    for n in seqs.into_iter().skip(KEEP_GENERATIONS) {
        if Some(n) != cur {
            let _ = std::fs::remove_dir_all(gens.join(n.to_string()));
        }
    }
}

/// Make every manifest path P a symlink to `<root>/current/files/P`.
/// Skips (with a warning) whatever it cannot do; returns the number linked.
/// Paths materialised last time that are gone from `m` are unlinked, but only when the
/// link still points into `<root>/current/files/` or `<root>/gens/`.
pub fn materialise(root: &Path, m: &Manifest) -> usize {
    let record = root.join("materialised.json");
    let prev: Vec<String> = std::fs::read(&record)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for path in prev.iter().filter(|p| !m.entries.contains_key(*p)) {
        if let Err(e) = unlink_ours(root, path) {
            eprintln!("carry: warning: cannot remove {path}: {e}");
        }
    }
    let keys: Vec<&String> = m.entries.keys().collect();
    if let Ok(b) = serde_json::to_vec(&keys) {
        let tmp = root.join(".materialised.json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &record);
        }
    }
    let skip = conflicting(m);
    let mut n = 0;
    for path in m.entries.keys().filter(|p| !skip.contains(*p)) {
        match materialise_one(root, path) {
            Ok(()) => n += 1,
            Err(e) => eprintln!("carry: warning: cannot materialise {path}: {e}"),
        }
    }
    n
}

/// Remove the symlink at `path` iff it is one of ours (points into our tree).
fn unlink_ours(root: &Path, path: &str) -> std::io::Result<()> {
    let p = Path::new(path);
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_symlink() => {
            let t = std::fs::read_link(p)?;
            if t.starts_with(root.join("current").join("files")) || t.starts_with(root.join("gens"))
            {
                std::fs::remove_file(p)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn materialise_one(root: &Path, path: &str) -> std::io::Result<()> {
    let r = rel(path).map_err(|e| std::io::Error::other(e.to_string()))?;
    let target = root.join("current").join("files").join(r);
    let p = Path::new(path);
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_symlink() => {
            if std::fs::read_link(p)? == target {
                return Ok(());
            }
            std::fs::remove_file(p)?;
        }
        Ok(md) if md.is_dir() => return Err(std::io::Error::other("is a directory; not replaced")),
        Ok(_) => std::fs::remove_file(p)?,
        Err(_) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
    }
    std::os::unix::fs::symlink(&target, p)
}

/// `carry status`: status.json plus tool readiness, per docs/cli-contract.md.
pub async fn status(s: &Settings) -> Result<Value> {
    let root = s.root.display().to_string();
    if s.repo.is_none() {
        return Ok(
            json!({"state": null, "mode": "disabled", "version": null, "root": root,
            "last_restore_ms": null, "error": null, "tools": []}),
        );
    }
    let st: Value = std::fs::read(s.root.join("status.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| {
            json!({"state": state_name(s), "mode": "degraded", "version": null,
                "last_restore_ms": null, "error": "no restore has run yet"})
        });
    // Written by tool prefetch: name -> {"status": "ready"|"failed", "reason"}.
    let prefetch: serde_json::Map<String, Value> =
        std::fs::read(s.root.join("cache/tools/status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    let tools: Vec<Value> = current_manifest(&s.root)
        .map(|m| {
            m.apps
                .keys()
                .map(|name| {
                    let p = prefetch.get(name);
                    if p.and_then(|v| v.get("status")).and_then(Value::as_str) == Some("failed") {
                        let reason = p
                            .and_then(|v| v.get("reason"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        return json!({"name": name, "status": "failed", "reason": reason});
                    }
                    let ready =
                        p.and_then(|v| v.get("status")).and_then(Value::as_str) == Some("ready");
                    json!({"name": name, "status": if ready { "ready" } else { "missing" }})
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "state": st.get("state").cloned().unwrap_or(Value::Null),
        "mode": st.get("mode").cloned().unwrap_or(json!("degraded")),
        "version": st.get("version").cloned().unwrap_or(Value::Null),
        "root": root,
        "last_restore_ms": st.get("last_restore_ms").cloned().unwrap_or(Value::Null),
        "error": st.get("error").cloned().unwrap_or(Value::Null),
        "tools": tools,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::INLINE_MAX;
    use crate::store::Store;
    use std::sync::Arc;

    pub(crate) async fn mem_repo() -> Repo {
        let s = Store::from_object_store(Arc::new(object_store::memory::InMemory::new()), "r");
        Repo::open_or_init(s, None, true).await.unwrap()
    }

    pub(crate) async fn commit_files(
        repo: &Repo,
        state: &str,
        files: &[(&str, &[u8])],
        env: &[(&str, &str)],
    ) {
        let files: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect();
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ctx::mutate(repo, state, "put", vec![], |mut m| {
            let files = files.clone();
            let env = env.clone();
            async move {
                for (p, b) in files {
                    let (blob, inline) = if b.len() < INLINE_MAX {
                        (
                            None,
                            Some(base64::engine::general_purpose::STANDARD.encode(&b)),
                        )
                    } else {
                        (Some(repo.put_blob(&b).await?), None)
                    };
                    m.entries.insert(
                        p,
                        Entry {
                            kind: EntryKind::File,
                            mode: 0o640,
                            size: b.len() as u64,
                            blob,
                            inline,
                            target: None,
                        },
                    );
                }
                for (k, v) in env {
                    m.env.insert(k, v);
                }
                Ok(m)
            }
        })
        .await
        .unwrap();
    }

    fn budget() -> Duration {
        Duration::from_secs(5)
    }

    #[tokio::test]
    async fn generation_flip_and_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = mem_repo().await;
        let big = vec![7u8; INLINE_MAX * 3];
        commit_files(&repo, "w", &[("/etc/a", b"one"), ("/opt/big", &big)], &[]).await;
        let r = restore_with(async { Ok(repo.clone()) }, "w", root, budget()).await;
        assert_eq!(
            (r.mode.clone(), r.version),
            (Mode::Ok, Some(1)),
            "{:?}",
            r.error
        );
        assert_eq!(
            std::fs::read(root.join("current/files/etc/a")).unwrap(),
            b"one"
        );
        assert_eq!(
            std::fs::read(root.join("current/files/opt/big")).unwrap(),
            big
        );
        let md = std::fs::metadata(root.join("current/files/etc/a")).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o640);
        let mm = std::fs::metadata(root.join("current/manifest.json")).unwrap();
        assert_eq!(mm.permissions().mode() & 0o777, 0o600);

        for i in 2..=4u8 {
            commit_files(&repo, "w", &[("/etc/a", &[b'0' + i])], &[]).await;
            let r = restore_with(async { Ok(repo.clone()) }, "w", root, budget()).await;
            assert_eq!(r.version, Some(i as u64));
        }
        assert_eq!(
            std::fs::read(root.join("current/files/etc/a")).unwrap(),
            b"4"
        );
        assert_eq!(current_generation(root), Some(4));
        let mut left: Vec<String> = std::fs::read_dir(root.join("gens"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into())
            .collect();
        left.sort();
        assert_eq!(left, vec!["3", "4"]);
        let st: Value =
            serde_json::from_slice(&std::fs::read(root.join("status.json")).unwrap()).unwrap();
        assert_eq!(st["mode"], "ok");
        assert_eq!(st["version"], 4);
    }

    #[tokio::test]
    async fn interrupted_tmp_is_ignored_and_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = mem_repo().await;
        commit_files(&repo, "w", &[("/etc/a", b"one")], &[]).await;
        restore_with(async { Ok(repo.clone()) }, "w", root, budget()).await;
        // Simulate a crash mid-restore of a later version.
        std::fs::create_dir_all(root.join("gens/9.tmp/files")).unwrap();
        assert_eq!(current_generation(root), Some(1));
        let r = restore_with(
            async { Err(CarryError::Unreachable("down".into())) },
            "w",
            root,
            budget(),
        )
        .await;
        assert_eq!((r.mode, r.version), (Mode::Degraded, Some(1)));
        assert!(!root.join("gens/9.tmp").exists());
        assert_eq!(
            std::fs::read(root.join("current/files/etc/a")).unwrap(),
            b"one"
        );
    }

    #[tokio::test]
    async fn degraded_unreachable_within_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Settings {
            repo: Some("s3://bucket/p".into()),
            endpoint: Some("http://127.0.0.1:1".into()),
            region: Some("us-east-1".into()),
            access_key_id: Some("x".into()),
            secret_access_key: Some("y".into()),
            root: tmp.path().into(),
            no_encryption: true,
            ..Default::default()
        };
        let t = Instant::now();
        let r = restore(&s, "w", tmp.path(), Duration::from_millis(500)).await;
        assert!(
            t.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(r.mode, Mode::Degraded);
        assert_eq!(current_generation(tmp.path()), Some(0));
        let st = status(&s).await.unwrap();
        assert_eq!(st["mode"], "degraded");
    }

    #[tokio::test]
    async fn materialise_links_only_manifest_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let fs = tmp.path().join("fs");
        std::fs::create_dir_all(&fs).unwrap();
        let a = fs.join("etc/a").display().to_string();
        let untouched = fs.join("other");
        std::fs::write(&untouched, b"x").unwrap();
        std::fs::create_dir_all(fs.join("etc")).unwrap();
        std::fs::write(&a, b"image").unwrap();
        let repo = mem_repo().await;
        commit_files(&repo, "w", &[(&a, b"v1")], &[]).await;
        restore_with(async { Ok(repo.clone()) }, "w", &root, budget()).await;
        assert_eq!(materialise(&root, &current_manifest(&root).unwrap()), 1);
        assert_eq!(std::fs::read(&a).unwrap(), b"v1");
        commit_files(&repo, "w", &[(&a, b"v2")], &[]).await;
        restore_with(async { Ok(repo.clone()) }, "w", &root, budget()).await;
        assert_eq!(std::fs::read(&a).unwrap(), b"v2");
        assert_eq!(std::fs::read(&untouched).unwrap(), b"x");
    }

    #[tokio::test]
    async fn materialise_removes_only_our_dropped_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let fs = tmp.path().join("fs");
        std::fs::create_dir_all(&fs).unwrap();
        let (a, b, c) = (fs.join("a"), fs.join("b"), fs.join("c"));
        let (sa, sb, sc) = (
            a.display().to_string(),
            b.display().to_string(),
            c.display().to_string(),
        );
        let repo = mem_repo().await;
        commit_files(&repo, "w", &[(&sa, b"1"), (&sb, b"2"), (&sc, b"3")], &[]).await;
        restore_with(async { Ok(repo.clone()) }, "w", &root, budget()).await;
        assert_eq!(materialise(&root, &current_manifest(&root).unwrap()), 3);
        // Someone repoints c elsewhere: not ours any more.
        let elsewhere = fs.join("elsewhere");
        std::fs::write(&elsewhere, b"mine").unwrap();
        std::fs::remove_file(&c).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &c).unwrap();
        crate::ops::rm_in(&repo, "w", &sb).await.unwrap();
        crate::ops::rm_in(&repo, "w", &sc).await.unwrap();
        restore_with(async { Ok(repo.clone()) }, "w", &root, budget()).await;
        assert_eq!(materialise(&root, &current_manifest(&root).unwrap()), 1);
        assert_eq!(std::fs::read(&a).unwrap(), b"1");
        assert!(
            std::fs::symlink_metadata(&b).is_err(),
            "dropped link removed"
        );
        assert_eq!(
            std::fs::read(&c).unwrap(),
            b"mine",
            "foreign link untouched"
        );
    }

    /// Peak RSS of this process in bytes.
    fn peak_rss() -> u64 {
        if let Ok(st) = std::fs::read_to_string("/proc/self/status") {
            if let Some(l) = st.lines().find(|l| l.starts_with("VmHWM:")) {
                let kb: u64 = l.split_whitespace().nth(1).unwrap().parse().unwrap();
                return kb * 1024;
            }
        }
        let ru = nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_SELF).unwrap();
        let v = ru.max_rss() as u64;
        if cfg!(target_os = "macos") {
            v
        } else {
            v * 1024
        }
    }

    /// Bounded memory (spec: container-runtime "Bounded memory"). Run alone, in release:
    /// `cargo test --release -- --ignored --test-threads=1 bounded_memory`
    #[tokio::test]
    #[ignore]
    async fn bounded_memory_put_and_restore_200mib() {
        const SIZE: usize = 200 << 20;
        let tmp = tempfile::tempdir().unwrap();
        let bucket = tmp.path().join("bucket");
        std::fs::create_dir_all(&bucket).unwrap();
        let store = Store::from_object_store(
            Arc::new(object_store::local::LocalFileSystem::new_with_prefix(&bucket).unwrap()),
            "r",
        );
        let mk = crate::crypto::MasterKey::from_bytes([9; 32]);
        let repo = Repo::open_or_init(store, Some(mk), false).await.unwrap();
        // Incompressible source file, written in 1 MiB pieces.
        let src = tmp.path().join("big.bin");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&src).unwrap());
            let mut buf = vec![0u8; 1 << 20];
            for _ in 0..(SIZE >> 20) {
                rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut buf);
                f.write_all(&buf).unwrap();
            }
        }
        let base = peak_rss();
        crate::ops::put_file_in(&repo, "w", "/data/big.bin", &src, 0o644)
            .await
            .unwrap();
        let after_put = peak_rss();
        let root = tmp.path().join("root");
        let r = restore_with(
            async { Ok(repo.clone()) },
            "w",
            &root,
            Duration::from_secs(600),
        )
        .await;
        assert_eq!(r.mode, Mode::Ok, "{:?}", r.error);
        let after_restore = peak_rss();
        let out = root.join("current/files/data/big.bin");
        assert_eq!(std::fs::metadata(&out).unwrap().len(), SIZE as u64);
        let mib = |b: u64| b as f64 / (1 << 20) as f64;
        println!(
            "RSS peak: base {:.1} MiB, after put {:.1} MiB (+{:.1}), after restore {:.1} MiB (+{:.1} total)",
            mib(base), mib(after_put), mib(after_put - base), mib(after_restore), mib(after_restore - base)
        );
        assert!(
            after_restore - base < 64 << 20,
            "peak RSS grew {:.1} MiB",
            mib(after_restore - base)
        );
    }

    fn inline(b: &[u8]) -> Entry {
        Entry {
            kind: EntryKind::File,
            mode: 0o644,
            size: b.len() as u64,
            blob: None,
            inline: Some(base64::engine::general_purpose::STANDARD.encode(b)),
            target: None,
        }
    }

    #[tokio::test]
    async fn legacy_bad_manifest_skips_conflicts_and_never_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let repo = mem_repo().await;
        // Written directly (bypassing the write-time check), as an older carry could have.
        let mut m = crate::repo::tests::manifest(1, None);
        m.entries.insert("/a".into(), inline(b"file"));
        m.entries.insert("/a/b".into(), inline(b"child"));
        m.entries.insert(
            "/x".into(),
            Entry {
                kind: EntryKind::Symlink,
                mode: 0o777,
                size: 0,
                blob: None,
                inline: None,
                target: Some(outside.display().to_string()),
            },
        );
        m.entries.insert("/x/passwd".into(), inline(b"pwned"));
        m.entries.insert("/ok".into(), inline(b"fine"));
        repo.commit("w", None, &m).await.unwrap();
        let r = restore_with(async { Ok(repo.clone()) }, "w", &root, budget()).await;
        assert_eq!(
            (r.mode.clone(), r.version),
            (Mode::Ok, Some(1)),
            "{:?}",
            r.error
        );
        assert_eq!(
            std::fs::read(root.join("current/files/ok")).unwrap(),
            b"fine"
        );
        assert!(
            !outside.join("passwd").exists(),
            "wrote through a symlink entry"
        );
        assert!(std::fs::symlink_metadata(root.join("current/files/a")).is_err());
        assert!(std::fs::symlink_metadata(root.join("current/files/x")).is_err());
    }

    #[test]
    fn writes_never_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("files");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("x")).unwrap();
        assert!(mkdirs_inside(&base, Path::new("x/y")).is_err());
        assert!(!outside.join("y").exists());
        std::os::unix::fs::symlink(outside.join("t"), base.join("f")).unwrap();
        assert!(write_file(&base.join("f"), b"z", 0o644).is_err());
        assert!(!outside.join("t").exists());
        mkdirs_inside(&base, Path::new("p/q")).unwrap();
        assert!(base.join("p/q").is_dir());
    }

    #[tokio::test]
    async fn head_rollback_below_high_water_mark_is_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = mem_repo().await;
        commit_files(&repo, "w", &[("/etc/a", b"one")], &[]).await;
        commit_files(&repo, "w", &[("/etc/a", b"two")], &[]).await;
        let r = restore_with(async { Ok(repo.clone()) }, "w", root, budget()).await;
        assert_eq!(r.version, Some(2));
        // An attacker replays the old HEAD (version 1).
        let v1 = crate::ctx::manifest_at(&repo, "w", 1).await.unwrap();
        let id = repo.write_manifest(&v1).await.unwrap();
        let head = serde_json::to_vec(&crate::model::Head {
            manifest: id,
            seq: 1,
        })
        .unwrap();
        repo.store.put("refs/w/HEAD", head.into()).await.unwrap();
        let r = restore_with(async { Ok(repo.clone()) }, "w", root, budget()).await;
        assert_eq!(
            (r.mode, r.version),
            (Mode::Degraded, Some(2)),
            "{:?}",
            r.error
        );
        assert_eq!(
            std::fs::read(root.join("current/files/etc/a")).unwrap(),
            b"two"
        );
        // A HEAD whose seq disagrees with its manifest is refused too.
        let v2 = crate::ctx::manifest_at(&repo, "w", 1).await.unwrap();
        let id = repo.write_manifest(&v2).await.unwrap();
        let head = serde_json::to_vec(&crate::model::Head {
            manifest: id,
            seq: 5,
        })
        .unwrap();
        repo.store.put("refs/w/HEAD", head.into()).await.unwrap();
        assert!(matches!(
            crate::ctx::current(&repo, "w").await,
            Err(CarryError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn status_reports_failed_prefetch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut m = crate::repo::tests::manifest(3, None);
        m.apps.insert("jq".into(), Default::default());
        m.apps.insert("yq".into(), Default::default());
        std::fs::create_dir_all(root.join("gens/3")).unwrap();
        std::fs::write(
            root.join("gens/3/manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        switch_current(root, 3).unwrap();
        std::fs::create_dir_all(root.join("cache/tools")).unwrap();
        std::fs::write(
            root.join("cache/tools/status.json"),
            br#"{"jq":{"status":"failed","reason":"bad signature"}}"#,
        )
        .unwrap();
        let s = Settings {
            repo: Some("s3://b/p".into()),
            root: root.into(),
            ..Default::default()
        };
        let st = status(&s).await.unwrap();
        assert_eq!(
            st["tools"][0],
            json!({"name": "jq", "status": "failed", "reason": "bad signature"})
        );
        assert_eq!(st["tools"][1]["status"], "missing");
    }

    #[tokio::test]
    async fn status_disabled_without_repo() {
        let s = Settings {
            root: "/nonexistent".into(),
            ..Default::default()
        };
        assert_eq!(status(&s).await.unwrap()["mode"], "disabled");
    }
}
