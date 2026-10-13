//! State commands: init, doctor, states, put/get/ls/rm/mv, log/diff/rollback, env.
//! Every write goes through `ctx::mutate`. Each command has a `Settings` wrapper
//! (used by the CLI) and a `*_in(&Repo, ..)` core (used by tests and other modules).
use crate::cli::GetOutput;
use crate::ctx::{self, current, manifest_at, mutate, open_repo, open_repo_for_write};
use crate::error::{CarryError, Result};
use crate::model::{Entry, EntryKind, Manifest, INLINE_MAX};
use crate::repo::{doctor_conditional_writes, Repo, CONFIG_KEY};
use crate::settings::Settings;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// ---------------------------------------------------------------- validation

pub fn check_state(state: &str) -> Result<()> {
    let ok = !state.is_empty()
        && state.len() <= 128
        && state.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !state.starts_with('.');
    if ok { Ok(()) } else { Err(CarryError::Usage(format!("invalid state name '{state}'"))) }
}

/// Absolute, normalised: starts with '/', no empty, '.' or '..' components, not '/'.
pub fn check_path(path: &str) -> Result<()> {
    let bad = || CarryError::Usage(format!("path must be absolute and normalised: '{path}'"));
    let rest = path.strip_prefix('/').ok_or_else(bad)?;
    if rest.is_empty() || path.contains('\0') {
        return Err(bad());
    }
    if rest.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(bad());
    }
    Ok(())
}

fn check_env_key(k: &str) -> Result<()> {
    let mut cs = k.chars();
    let ok = cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok { Ok(()) } else { Err(CarryError::Usage(format!("invalid env var name '{k}'"))) }
}

fn not_found_state(state: &str) -> CarryError {
    CarryError::NotFound(format!("state '{state}'"))
}

// ---------------------------------------------------------------- repo-level

pub async fn init(s: &Settings) -> Result<Value> {
    let store = ctx::store_from(s)?;
    let existed = store.get_opt(CONFIG_KEY).await?.is_some();
    let repo = open_repo_for_write(s).await?;
    Ok(json!({"repo": s.repo, "created": !existed, "encrypted": repo.is_encrypted()}))
}

pub async fn doctor(s: &Settings) -> Result<Value> {
    let store = ctx::store_from(s)?;
    let mut checks = Vec::new();
    let (reachable, config) = match store.get_opt(CONFIG_KEY).await {
        Ok(c) => {
            checks.push(json!({"name": "reachable", "ok": true, "detail": "store answered"}));
            (true, c)
        }
        Err(e) => {
            checks.push(json!({"name": "reachable", "ok": false, "detail": e.to_string()}));
            (false, None)
        }
    };
    let mut conditional = false;
    let mut encrypted = Value::Null;
    if reachable {
        match doctor_conditional_writes(&store).await {
            Ok(r) => {
                conditional = r.if_none_match && r.if_match;
                let detail = if r.detail.is_empty() { "If-None-Match and If-Match honoured".into() } else { r.detail.join("; ") };
                checks.push(json!({"name": "conditional_writes", "ok": conditional, "detail": detail}));
            }
            Err(e) => checks.push(json!({"name": "conditional_writes", "ok": false, "detail": e.to_string()})),
        }
        match config {
            None => checks.push(json!({"name": "initialised", "ok": false, "detail": "no repository config; run `carry init`"})),
            Some(raw) => {
                let enc = serde_json::from_slice::<Value>(&raw).ok().and_then(|v| v.get("encrypted").and_then(Value::as_bool));
                checks.push(json!({"name": "initialised", "ok": enc.is_some(), "detail": if enc.is_some() { "config present" } else { "config unreadable" }}));
                if let Some(e) = enc {
                    encrypted = json!(e);
                    let (ok, detail) = match open_repo(s).await {
                        Ok(_) => (true, if e { "master key opens repository".to_string() } else { "repository is unencrypted".to_string() }),
                        Err(err) => (false, err.to_string()),
                    };
                    checks.push(json!({"name": "key", "ok": ok, "detail": detail}));
                }
            }
        }
    }
    Ok(json!({"reachable": reachable, "conditional_writes": conditional, "encrypted": encrypted, "checks": checks}))
}

pub async fn states(s: &Settings) -> Result<Value> {
    states_in(&open_repo(s).await?).await
}

pub async fn states_in(repo: &Repo) -> Result<Value> {
    let mut out = Vec::new();
    for name in repo.list_states().await? {
        if let Some((h, _)) = repo.read_head(&name).await? {
            out.push(json!({"name": name, "version": h.seq}));
        }
    }
    Ok(json!({"states": out}))
}

// ---------------------------------------------------------------- files

pub async fn put(s: &Settings, state: &str, path: &str, bytes: Vec<u8>, mode: u32) -> Result<Value> {
    check_state(state)?;
    check_path(path)?;
    put_in(&open_repo_for_write(s).await?, state, path, bytes, mode).await
}

pub async fn put_in(repo: &Repo, state: &str, path: &str, bytes: Vec<u8>, mode: u32) -> Result<Value> {
    check_state(state)?;
    check_path(path)?;
    let size = bytes.len() as u64;
    let (entry, uploaded) = if bytes.len() < INLINE_MAX {
        (Entry { kind: EntryKind::File, mode, size, blob: None, inline: Some(B64.encode(&bytes)), target: None }, false)
    } else {
        let (id, up) = repo.put_blob_report(&bytes).await?;
        (Entry { kind: EntryKind::File, mode, size, blob: Some(id), inline: None, target: None }, up)
    };
    let head = mutate(repo, state, "put", vec![path.to_string()], |mut m: Manifest| {
        let e = entry.clone();
        async move {
            m.entries.insert(path.to_string(), e);
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "path": path, "version": head.seq, "size": size, "uploaded": uploaded}))
}

/// `put` from a file on disk. Large files stream (hash pass + bounded chunked upload)
/// instead of being read into memory.
pub async fn put_file(s: &Settings, state: &str, path: &str, file: &std::path::Path, mode: u32) -> Result<Value> {
    check_state(state)?;
    check_path(path)?;
    put_file_in(&open_repo_for_write(s).await?, state, path, file, mode).await
}

pub async fn put_file_in(repo: &Repo, state: &str, path: &str, file: &std::path::Path, mode: u32) -> Result<Value> {
    check_state(state)?;
    check_path(path)?;
    let ue = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => CarryError::NotFound(file.display().to_string()),
        _ => CarryError::Usage(format!("cannot read {}: {e}", file.display())),
    };
    let f = std::fs::File::open(file).map_err(ue)?;
    let size = f.metadata().map_err(ue)?.len();
    if size <= crate::crypto::CHUNK_THRESHOLD {
        let bytes = std::fs::read(file).map_err(ue)?;
        return put_in(repo, state, path, bytes, mode).await;
    }
    let (id, uploaded) = repo.put_blob_from(f, size).await?;
    let entry = Entry { kind: EntryKind::File, mode, size, blob: Some(id), inline: None, target: None };
    let head = mutate(repo, state, "put", vec![path.to_string()], |mut m: Manifest| {
        let e = entry.clone();
        async move {
            m.entries.insert(path.to_string(), e);
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "path": path, "version": head.seq, "size": size, "uploaded": uploaded}))
}

pub async fn get(s: &Settings, state: &str, path: &str) -> Result<GetOutput> {
    check_state(state)?;
    check_path(path)?;
    get_in(&open_repo(s).await?, state, path).await
}

pub async fn get_in(repo: &Repo, state: &str, path: &str) -> Result<GetOutput> {
    let (_, _, m) = current(repo, state).await?.ok_or_else(|| not_found_state(state))?;
    let e = m.entries.get(path).ok_or_else(|| CarryError::NotFound(format!("{state}:{path}")))?;
    let bytes = match (&e.kind, &e.inline, &e.blob, &e.target) {
        (EntryKind::Symlink, _, _, Some(t)) => t.as_bytes().to_vec(),
        (_, Some(b), _, _) => B64.decode(b).map_err(|e| CarryError::Invalid(format!("inline content: {e}")))?,
        (_, _, Some(id), _) => repo.get_blob(id).await?.to_vec(),
        _ => return Err(CarryError::Invalid(format!("entry {path} has no content"))),
    };
    Ok(GetOutput { bytes, mode: e.mode })
}

pub async fn ls(s: &Settings, state: &str, dir: Option<&str>) -> Result<Value> {
    check_state(state)?;
    ls_in(&open_repo(s).await?, state, dir).await
}

pub async fn ls_in(repo: &Repo, state: &str, dir: Option<&str>) -> Result<Value> {
    let prefix = match dir.map(|d| d.trim_end_matches('/')) {
        None | Some("") => None,
        Some(d) => {
            check_path(d)?;
            Some(d.to_string())
        }
    };
    let (head, _, m) = current(repo, state).await?.ok_or_else(|| not_found_state(state))?;
    let entries: Vec<Value> = m
        .entries
        .iter()
        .filter(|(p, _)| match &prefix {
            None => true,
            Some(d) => p.as_str() == d || p.strip_prefix(d.as_str()).is_some_and(|r| r.starts_with('/')),
        })
        .map(|(p, e)| {
            let kind = match e.kind { EntryKind::File => "file", EntryKind::Symlink => "symlink" };
            json!({"path": p, "kind": kind, "size": e.size, "mode": e.mode})
        })
        .collect();
    Ok(json!({"state": state, "version": head.seq, "entries": entries}))
}

pub async fn rm(s: &Settings, state: &str, path: &str) -> Result<Value> {
    check_state(state)?;
    check_path(path)?;
    rm_in(&open_repo_for_write(s).await?, state, path).await
}

pub async fn rm_in(repo: &Repo, state: &str, path: &str) -> Result<Value> {
    check_path(path)?;
    if current(repo, state).await?.is_none() {
        return Err(not_found_state(state));
    }
    let head = mutate(repo, state, "rm", vec![path.to_string()], |mut m: Manifest| async move {
        m.entries.remove(path).ok_or_else(|| CarryError::NotFound(format!("{state}:{path}")))?;
        Ok(m)
    })
    .await?;
    Ok(json!({"state": state, "path": path, "version": head.seq}))
}

pub async fn mv(s: &Settings, state: &str, from: &str, to: &str) -> Result<Value> {
    check_state(state)?;
    check_path(from)?;
    check_path(to)?;
    mv_in(&open_repo_for_write(s).await?, state, from, to).await
}

pub async fn mv_in(repo: &Repo, state: &str, from: &str, to: &str) -> Result<Value> {
    check_path(from)?;
    check_path(to)?;
    if from == to {
        return Err(CarryError::Usage("mv: source and destination are the same".into()));
    }
    if current(repo, state).await?.is_none() {
        return Err(not_found_state(state));
    }
    let args = vec![from.to_string(), to.to_string()];
    let head = mutate(repo, state, "mv", args, |mut m: Manifest| async move {
        if m.entries.contains_key(to) {
            return Err(CarryError::Usage(format!("mv: {state}:{to} already exists")));
        }
        let e = m.entries.remove(from).ok_or_else(|| CarryError::NotFound(format!("{state}:{from}")))?;
        m.entries.insert(to.to_string(), e);
        Ok(m)
    })
    .await?;
    Ok(json!({"state": state, "from": from, "to": to, "version": head.seq}))
}

// ---------------------------------------------------------------- history

pub async fn log(s: &Settings, state: &str, limit: Option<usize>) -> Result<Value> {
    check_state(state)?;
    log_in(&open_repo(s).await?, state, limit).await
}

pub async fn log_in(repo: &Repo, state: &str, limit: Option<usize>) -> Result<Value> {
    let limit = limit.unwrap_or(usize::MAX);
    let (_, _, mut m) = current(repo, state).await?.ok_or_else(|| not_found_state(state))?;
    let mut out = Vec::new();
    while out.len() < limit {
        out.push(json!({
            "version": m.seq,
            "op": m.op.kind,
            "args": m.op.args,
            "at": m.op.at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "by": m.op.by,
        }));
        match m.parent.clone() {
            Some(p) => m = repo.load_manifest(&p).await?,
            None => break,
        }
    }
    Ok(json!({"state": state, "versions": out}))
}

fn changed_keys<V: PartialEq>(a: &BTreeMap<String, V>, b: &BTreeMap<String, V>) -> Vec<String> {
    let mut v: Vec<String> = a
        .iter()
        .filter(|(k, va)| b.get(*k) != Some(va))
        .map(|(k, _)| k.clone())
        .chain(b.keys().filter(|k| !a.contains_key(*k)).cloned())
        .collect();
    v.sort();
    v
}

pub async fn diff(s: &Settings, state: &str, v1: u64, v2: u64) -> Result<Value> {
    check_state(state)?;
    diff_in(&open_repo(s).await?, state, v1, v2).await
}

pub async fn diff_in(repo: &Repo, state: &str, v1: u64, v2: u64) -> Result<Value> {
    let a = manifest_at(repo, state, v1).await?;
    let b = manifest_at(repo, state, v2).await?;
    let added: Vec<&String> = b.entries.keys().filter(|k| !a.entries.contains_key(*k)).collect();
    let removed: Vec<&String> = a.entries.keys().filter(|k| !b.entries.contains_key(*k)).collect();
    let changed: Vec<&String> = a.entries.iter().filter(|(k, e)| b.entries.get(*k).is_some_and(|f| f != *e)).map(|(k, _)| k).collect();
    Ok(json!({
        "added": added,
        "removed": removed,
        "changed": changed,
        "env_changed": changed_keys(&a.env, &b.env),
        "apps_changed": changed_keys(&a.apps, &b.apps),
    }))
}

pub async fn rollback(s: &Settings, state: &str, version: u64) -> Result<Value> {
    check_state(state)?;
    rollback_in(&open_repo_for_write(s).await?, state, version).await
}

pub async fn rollback_in(repo: &Repo, state: &str, version: u64) -> Result<Value> {
    if version == 0 {
        return Err(CarryError::Usage("versions start at 1".into()));
    }
    let target = manifest_at(repo, state, version).await?;
    let head = mutate(repo, state, "rollback", vec![format!("v{version}")], |mut m: Manifest| {
        let t = target.clone();
        async move {
            m.entries = t.entries;
            m.env = t.env;
            m.apps = t.apps;
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "version": head.seq, "restored_from": version}))
}

// ---------------------------------------------------------------- env

pub async fn env_set(s: &Settings, state: &str, kv: Vec<(String, String)>) -> Result<Value> {
    check_state(state)?;
    env_set_in(&open_repo_for_write(s).await?, state, kv, "env_set").await
}

async fn env_set_in(repo: &Repo, state: &str, kv: Vec<(String, String)>, kind: &str) -> Result<Value> {
    check_state(state)?;
    for (k, _) in &kv {
        check_env_key(k)?;
    }
    let keys: Vec<String> = kv.iter().map(|(k, _)| k.clone()).collect();
    // Op args carry key names only: values may be secrets.
    let head = mutate(repo, state, kind, keys.clone(), |mut m: Manifest| {
        let kv = kv.clone();
        async move {
            m.env.extend(kv);
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "version": head.seq, "keys": keys}))
}

pub async fn env_unset(s: &Settings, state: &str, keys: Vec<String>) -> Result<Value> {
    check_state(state)?;
    env_unset_in(&open_repo_for_write(s).await?, state, keys).await
}

pub async fn env_unset_in(repo: &Repo, state: &str, keys: Vec<String>) -> Result<Value> {
    for k in &keys {
        check_env_key(k)?;
    }
    if current(repo, state).await?.is_none() {
        return Err(not_found_state(state));
    }
    let head = mutate(repo, state, "env_unset", keys.clone(), |mut m: Manifest| {
        let keys = keys.clone();
        async move {
            for k in &keys {
                m.env.remove(k);
            }
            Ok(m)
        }
    })
    .await?;
    Ok(json!({"state": state, "version": head.seq, "keys": keys}))
}

pub async fn env_import(s: &Settings, state: &str, text: &str) -> Result<Value> {
    check_state(state)?;
    let kv = parse_dotenv(text)?;
    env_set_in(&open_repo_for_write(s).await?, state, kv, "env_import").await
}

#[allow(dead_code)]
pub async fn env_import_in(repo: &Repo, state: &str, text: &str) -> Result<Value> {
    env_set_in(repo, state, parse_dotenv(text)?, "env_import").await
}

pub async fn env_get(s: &Settings, state: &str, key: Option<&str>) -> Result<Value> {
    check_state(state)?;
    env_get_in(&open_repo(s).await?, state, key).await
}

pub async fn env_get_in(repo: &Repo, state: &str, key: Option<&str>) -> Result<Value> {
    let (_, _, m) = current(repo, state).await?.ok_or_else(|| not_found_state(state))?;
    let env = match key {
        None => m.env,
        Some(k) => {
            let v = m.env.get(k).ok_or_else(|| CarryError::NotFound(format!("env {k} in '{state}'")))?;
            BTreeMap::from([(k.to_string(), v.clone())])
        }
    };
    Ok(json!({"state": state, "env": env}))
}

/// Parse dotenv text: `KEY=VALUE`, optional `export ` prefix, `#` comments,
/// blank lines, single quotes (literal) and double quotes (\n \t \" \\ escapes).
/// Later duplicates win; order of first appearance is kept.
pub fn parse_dotenv(text: &str) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        let err = || CarryError::Invalid(format!("dotenv line {}: expected KEY=VALUE", i + 1));
        let (k, v) = line.split_once('=').ok_or_else(err)?;
        let k = k.trim();
        check_env_key(k).map_err(|_| err())?;
        let v = v.trim();
        let val = if let Some(rest) = v.strip_prefix('"') {
            let mut s = String::new();
            let mut chars = rest.chars();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some(o) => s.push(o),
                        None => break,
                    },
                    c => s.push(c),
                }
            }
            if !closed {
                return Err(CarryError::Invalid(format!("dotenv line {}: unterminated quote", i + 1)));
            }
            s
        } else if let Some(rest) = v.strip_prefix('\'') {
            let end = rest.find('\'').ok_or_else(|| CarryError::Invalid(format!("dotenv line {}: unterminated quote", i + 1)))?;
            rest[..end].to_string()
        } else {
            match v.find(" #") {
                Some(p) => v[..p].trim_end().to_string(),
                None => v.to_string(),
            }
        };
        match out.iter_mut().find(|(ek, _)| ek == k) {
            Some(e) => e.1 = val,
            None => out.push((k.to_string(), val)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::MasterKey;
    use crate::store::Store;
    use std::sync::Arc;

    async fn repo() -> Repo {
        let s = Store::from_object_store(Arc::new(object_store::memory::InMemory::new()), "");
        Repo::open_or_init(s, Some(MasterKey::generate()), false).await.unwrap()
    }

    #[test]
    fn validation() {
        assert!(check_path("/etc/a.yaml").is_ok());
        for bad in ["etc/a", "/", "/a/../b", "/a/./b", "/a//b", "/a/", ""] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        assert!(check_state("web-1").is_ok());
        assert!(check_state(".x").is_err());
        assert!(check_state("a/b").is_err());
    }

    #[tokio::test]
    async fn put_get_inline_and_blob_with_dedup() {
        let r = repo().await;
        let v = put_in(&r, "w", "/etc/small", b"hi".to_vec(), 0o600).await.unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["uploaded"], false);
        let big = vec![7u8; INLINE_MAX + 10];
        let v = put_in(&r, "w", "/big", big.clone(), 0o644).await.unwrap();
        assert_eq!((v["version"].as_u64(), v["uploaded"].as_bool()), (Some(2), Some(true)));
        let v = put_in(&r, "w", "/big2", big.clone(), 0o644).await.unwrap();
        assert_eq!(v["uploaded"], false, "dedup");
        let g = get_in(&r, "w", "/etc/small").await.unwrap();
        assert_eq!((g.bytes.as_slice(), g.mode), (&b"hi"[..], 0o600));
        assert_eq!(get_in(&r, "w", "/big").await.unwrap().bytes, big);
        let (_, _, m) = current(&r, "w").await.unwrap().unwrap();
        assert!(m.entries["/etc/small"].inline.is_some() && m.entries["/big"].blob.is_some());
        assert!(matches!(get_in(&r, "w", "/nope").await, Err(CarryError::NotFound(_))));
        let l = ls_in(&r, "w", Some("/etc")).await.unwrap();
        assert_eq!(l["entries"].as_array().unwrap().len(), 1);
        assert_eq!(ls_in(&r, "w", None).await.unwrap()["entries"].as_array().unwrap().len(), 3);
        assert_eq!(states_in(&r).await.unwrap()["states"][0]["version"], 3);
    }

    #[tokio::test]
    async fn rm_mv_log_diff_rollback() {
        let r = repo().await;
        put_in(&r, "w", "/a", b"1".to_vec(), 0o644).await.unwrap(); // v1
        put_in(&r, "w", "/b", b"2".to_vec(), 0o644).await.unwrap(); // v2
        env_set_in(&r, "w", vec![("K".into(), "v".into())], "env_set").await.unwrap(); // v3
        mv_in(&r, "w", "/a", "/c").await.unwrap(); // v4
        assert!(matches!(rm_in(&r, "w", "/zzz").await, Err(CarryError::NotFound(_))));
        assert!(matches!(rm_in(&r, "nostate", "/a").await, Err(CarryError::NotFound(_))));
        assert_eq!(rm_in(&r, "w", "/b").await.unwrap()["version"], 5);
        put_in(&r, "w", "/c", b"changed".to_vec(), 0o644).await.unwrap(); // v6

        let log = log_in(&r, "w", None).await.unwrap();
        let ops: Vec<&str> = log["versions"].as_array().unwrap().iter().map(|v| v["op"].as_str().unwrap()).collect();
        assert_eq!(ops, ["put", "rm", "mv", "env_set", "put", "put"]);
        assert_eq!(log["versions"][0]["version"], 6);
        assert_eq!(log_in(&r, "w", Some(2)).await.unwrap()["versions"].as_array().unwrap().len(), 2);

        let d = diff_in(&r, "w", 2, 6).await.unwrap();
        assert_eq!(d["added"], json!(["/c"]));
        assert_eq!(d["removed"], json!(["/a", "/b"]));
        assert_eq!(d["env_changed"], json!(["K"]));
        let d = diff_in(&r, "w", 4, 6).await.unwrap();
        assert_eq!((d["changed"].clone(), d["removed"].clone()), (json!(["/c"]), json!(["/b"])));

        let rb = rollback_in(&r, "w", 2).await.unwrap();
        assert_eq!((rb["version"].as_u64(), rb["restored_from"].as_u64()), (Some(7), Some(2)));
        let d = diff_in(&r, "w", 2, 7).await.unwrap();
        for k in ["added", "removed", "changed", "env_changed", "apps_changed"] {
            assert_eq!(d[k], json!([]), "{k}");
        }
        let top = &log_in(&r, "w", Some(1)).await.unwrap()["versions"][0];
        assert_eq!((top["op"].clone(), top["args"].clone()), (json!("rollback"), json!(["v2"])));
        assert!(matches!(rollback_in(&r, "w", 99).await, Err(CarryError::NotFound(_))));
    }

    #[test]
    fn dotenv_parsing() {
        let t = "# c\n\nexport A=1\nB = \"x y\\n\\\"q\\\"\" \nC='lit $X # no'\nD=plain # comment\nE=\nA=2\n";
        let kv = parse_dotenv(t).unwrap();
        let m: BTreeMap<_, _> = kv.into_iter().collect();
        assert_eq!(m["A"], "2");
        assert_eq!(m["B"], "x y\n\"q\"");
        assert_eq!(m["C"], "lit $X # no");
        assert_eq!(m["D"], "plain");
        assert_eq!(m["E"], "");
        assert!(parse_dotenv("NOEQ\n").is_err());
        assert!(parse_dotenv("1X=a\n").is_err());
        assert!(parse_dotenv("A=\"open\n").is_err());
    }

    #[tokio::test]
    async fn env_import_get_unset() {
        let r = repo().await;
        let v = env_import_in(&r, "w", "A=1\nB=2\n").await.unwrap();
        assert_eq!(v["keys"], json!(["A", "B"]));
        assert_eq!(env_get_in(&r, "w", Some("B")).await.unwrap()["env"], json!({"B": "2"}));
        env_unset_in(&r, "w", vec!["A".into()]).await.unwrap();
        assert_eq!(env_get_in(&r, "w", None).await.unwrap()["env"], json!({"B": "2"}));
        let top = &log_in(&r, "w", Some(1)).await.unwrap()["versions"][0];
        assert_eq!(top["args"], json!(["A"]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_puts_cas_retry() {
        let r = Arc::new(repo().await);
        put_in(&r, "w", "/seed", b"0".to_vec(), 0o644).await.unwrap();
        let mut hs = Vec::new();
        for i in 0..2 {
            let r = r.clone();
            hs.push(tokio::spawn(async move { put_in(&r, "w", &format!("/f{i}"), vec![i as u8], 0o644).await }));
        }
        let mut vs: Vec<u64> = Vec::new();
        for h in hs {
            vs.push(h.await.unwrap().unwrap()["version"].as_u64().unwrap());
        }
        vs.sort();
        assert_eq!(vs, [2, 3]);
        let l = ls_in(&r, "w", None).await.unwrap();
        assert_eq!(l["entries"].as_array().unwrap().len(), 3);
    }
}
