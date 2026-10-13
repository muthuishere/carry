//! Shared plumbing for ops, restore, run and tools: open the repo from Settings,
//! and apply a mutation to a state as a committed version with conflict retry.
#![allow(dead_code)]
use crate::crypto::MasterKey;
use crate::error::{CarryError, Result};
use crate::model::{Head, Manifest, Op, FORMAT_VERSION};
use crate::repo::Repo;
use crate::settings::Settings;
use crate::store::{Store, StoreConfig};
use std::collections::BTreeMap;

/// Commit attempts are bounded by count and by wall time; backoff grows
/// exponentially with full jitter so many concurrent writers spread out.
pub const MAX_COMMIT_RETRIES: usize = 40;
pub const COMMIT_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

pub fn store_from(s: &Settings) -> Result<Store> {
    let url = s.repo.as_deref().ok_or_else(|| {
        CarryError::Usage("no repo configured (set --repo, CARRY_REPO or a profile)".into())
    })?;
    let mut cfg = StoreConfig::from_repo_url(url)?;
    cfg.endpoint = s.endpoint.clone();
    cfg.region = s.region.clone();
    cfg.access_key = s.access_key_id.clone();
    cfg.secret_key = s.secret_access_key.clone();
    if cfg
        .endpoint
        .as_deref()
        .is_some_and(|e| e.starts_with("http://"))
    {
        cfg.allow_http = true;
    }
    Store::new(&cfg)
}

fn master_key(s: &Settings) -> Result<Option<MasterKey>> {
    s.master_key
        .as_deref()
        .map(MasterKey::from_base64)
        .transpose()
}

/// Open an existing repo (read paths). NotFound if never initialised.
pub async fn open_repo(s: &Settings) -> Result<Repo> {
    Repo::open(store_from(s)?, master_key(s)?).await
}

/// Open, creating the repo on first write.
pub async fn open_repo_for_write(s: &Settings) -> Result<Repo> {
    Repo::open_or_init(store_from(s)?, master_key(s)?, s.no_encryption).await
}

/// Current manifest of a state, or None if the state has no versions yet.
pub async fn current(repo: &Repo, state: &str) -> Result<Option<(Head, String, Manifest)>> {
    match repo.read_head(state).await? {
        None => Ok(None),
        Some((head, etag)) => {
            let m = repo.load_manifest(state, &head.manifest).await?;
            if m.seq != head.seq {
                return Err(CarryError::Invalid(format!(
                    "HEAD of '{state}' says version {} but its manifest is version {}",
                    head.seq, m.seq
                )));
            }
            Ok(Some((head, etag, m)))
        }
    }
}

pub fn empty_manifest(op: Op) -> Manifest {
    Manifest {
        format: FORMAT_VERSION,
        parent: None,
        seq: 0,
        op,
        entries: BTreeMap::new(),
        env: BTreeMap::new(),
        apps: BTreeMap::new(),
        state: String::new(),
    }
}

/// Another entry that makes `path` unrestorable: an ancestor (a file or symlink cannot also
/// be a directory, and a symlink must never be traversed) or a descendant.
pub fn path_conflict<V>(entries: &BTreeMap<String, V>, path: &str) -> Option<String> {
    let mut p = path;
    while let Some(i) = p.rfind('/') {
        p = &p[..i];
        if !p.is_empty() && entries.contains_key(p) {
            return Some(p.to_string());
        }
    }
    let dir = format!("{path}/");
    entries
        .range(dir.clone()..)
        .next()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(&dir))
        .cloned()
}

/// Refuse entries added or changed by an op that would conflict with another entry.
fn check_new_entries(base: &Manifest, next: &Manifest) -> Result<()> {
    for (p, e) in &next.entries {
        if base.entries.get(p) == Some(e) {
            continue;
        }
        if let Some(other) = path_conflict(&next.entries, p) {
            return Err(CarryError::Usage(format!(
                "{p} conflicts with existing entry {other}: a path cannot be both a file/symlink and a directory"
            )));
        }
    }
    Ok(())
}

pub fn make_op(kind: &str, args: Vec<String>) -> Op {
    let by = std::env::var("CARRY_ACTOR")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into());
    Op {
        kind: kind.into(),
        args,
        at: chrono::Utc::now(),
        by,
    }
}

/// Load HEAD, let `f` edit a copy of the manifest, upload, and move HEAD with
/// compare-and-swap. Retries on conflict by re-reading HEAD and re-running `f`.
/// `f` must be deterministic given the manifest (it may upload blobs via the repo).
pub async fn mutate<F, Fut>(
    repo: &Repo,
    state: &str,
    kind: &str,
    args: Vec<String>,
    mut f: F,
) -> Result<Head>
where
    F: FnMut(Manifest) -> Fut,
    Fut: std::future::Future<Output = Result<Manifest>>,
{
    let mut last = None;
    let started = std::time::Instant::now();
    for attempt in 0..MAX_COMMIT_RETRIES {
        if attempt > 0 && started.elapsed() > COMMIT_RETRY_BUDGET {
            break;
        }
        let (base, etag) = match current(repo, state).await? {
            Some((head, etag, m)) => {
                let mut m = m;
                m.parent = Some(head.manifest.clone());
                m.seq = head.seq + 1;
                (m, Some(etag))
            }
            None => {
                let mut m = empty_manifest(make_op(kind, args.clone()));
                m.seq = 1;
                (m, None)
            }
        };
        let mut next = f(base.clone()).await?;
        check_new_entries(&base, &next)?;
        next.op = make_op(kind, args.clone());
        next.format = FORMAT_VERSION;
        match repo.commit(state, etag, &next).await {
            Ok(h) => return Ok(h),
            Err(CarryError::Conflict(msg)) => {
                last = Some(msg);
                let cap = (25u64 << attempt.min(6)).min(1500);
                let jitter = 10 + rand::random::<u64>() % cap;
                tokio::time::sleep(std::time::Duration::from_millis(jitter)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(CarryError::Conflict(
        last.unwrap_or_else(|| "too many concurrent writers".into()),
    ))
}

/// Walk back from HEAD to find the manifest with a given seq.
pub async fn manifest_at(repo: &Repo, state: &str, seq: u64) -> Result<Manifest> {
    let (head, _, mut m) = current(repo, state)
        .await?
        .ok_or_else(|| CarryError::NotFound(format!("state '{state}'")))?;
    if seq > head.seq {
        return Err(CarryError::NotFound(format!("version {seq} of '{state}'")));
    }
    while m.seq != seq {
        let p = m
            .parent
            .clone()
            .ok_or_else(|| CarryError::NotFound(format!("version {seq} of '{state}'")))?;
        m = repo.load_manifest(state, &p).await?;
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicts() {
        let e: BTreeMap<String, ()> = [
            ("/a".to_string(), ()),
            ("/c/d/e".to_string(), ()),
            ("/ab".to_string(), ()),
        ]
        .into_iter()
        .collect();
        assert_eq!(path_conflict(&e, "/a/b"), Some("/a".into()));
        assert_eq!(path_conflict(&e, "/c"), Some("/c/d/e".into()));
        assert_eq!(path_conflict(&e, "/c/d"), Some("/c/d/e".into()));
        assert_eq!(path_conflict(&e, "/a"), None);
        assert_eq!(path_conflict(&e, "/abc"), None);
    }
}
