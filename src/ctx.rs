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

pub const MAX_COMMIT_RETRIES: usize = 5;

pub fn store_from(s: &Settings) -> Result<Store> {
    let url = s
        .repo
        .as_deref()
        .ok_or_else(|| CarryError::Usage("no repo configured (set --repo, CARRY_REPO or a profile)".into()))?;
    let mut cfg = StoreConfig::from_repo_url(url)?;
    cfg.endpoint = s.endpoint.clone();
    cfg.region = s.region.clone();
    cfg.access_key = s.access_key_id.clone();
    cfg.secret_key = s.secret_access_key.clone();
    if cfg.endpoint.as_deref().is_some_and(|e| e.starts_with("http://")) {
        cfg.allow_http = true;
    }
    Store::new(&cfg)
}

fn master_key(s: &Settings) -> Result<Option<MasterKey>> {
    s.master_key.as_deref().map(MasterKey::from_base64).transpose()
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
            let m = repo.load_manifest(&head.manifest).await?;
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
    }
}

pub fn make_op(kind: &str, args: Vec<String>) -> Op {
    let by = std::env::var("CARRY_ACTOR")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into());
    Op { kind: kind.into(), args, at: chrono::Utc::now(), by }
}

/// Load HEAD, let `f` edit a copy of the manifest, upload, and move HEAD with
/// compare-and-swap. Retries on conflict by re-reading HEAD and re-running `f`.
/// `f` must be deterministic given the manifest (it may upload blobs via the repo).
pub async fn mutate<F, Fut>(repo: &Repo, state: &str, kind: &str, args: Vec<String>, mut f: F) -> Result<Head>
where
    F: FnMut(Manifest) -> Fut,
    Fut: std::future::Future<Output = Result<Manifest>>,
{
    let mut last = None;
    for attempt in 0..MAX_COMMIT_RETRIES {
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
        let mut next = f(base).await?;
        next.op = make_op(kind, args.clone());
        next.format = FORMAT_VERSION;
        match repo.commit(state, etag, &next).await {
            Ok(h) => return Ok(h),
            Err(CarryError::Conflict(msg)) => {
                last = Some(msg);
                let jitter = 20 + (rand::random::<u64>() % 80) * (attempt as u64 + 1);
                tokio::time::sleep(std::time::Duration::from_millis(jitter)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(CarryError::Conflict(last.unwrap_or_else(|| "too many concurrent writers".into())))
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
        let p = m.parent.clone().ok_or_else(|| CarryError::NotFound(format!("version {seq} of '{state}'")))?;
        m = repo.load_manifest(&p).await?;
    }
    Ok(m)
}
