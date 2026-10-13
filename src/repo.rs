//! Repository: config, blobs, manifests, HEAD, lease, tool-store helpers (ADR 0003/0004/0007).
//! See openspec/changes/carry-v0-1/design.md for this module's surface.
#![allow(dead_code)]

use crate::crypto::{Cipher, DataKey, MasterKey, WrappedKey};
use crate::crypto::{frame_header, CHUNK_SIZE, CHUNK_THRESHOLD, FMT_CHUNKED};
use crate::error::{CarryError, Result};
use crate::model::{Head, Manifest, FORMAT_VERSION};
use crate::store::Store;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const CONFIG_KEY: &str = "config";
pub const LEASE_TTL_SECS: i64 = 15;
/// Multipart part size for chunked uploads (S3 minimum is 5 MiB).
const PART_SIZE: usize = 8 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoConfig {
    pub format: u32,
    pub encrypted: bool,
    #[serde(default)]
    pub cipher: String,
    #[serde(default)]
    pub wrapped_keys: Vec<WrappedKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseRecord {
    pub holder: String,
    pub expires: DateTime<Utc>,
}

/// A held lease. Keep it and pass to renew/release.
#[derive(Debug, Clone)]
pub struct Lease {
    pub state: String,
    pub holder: String,
    pub expires: DateTime<Utc>,
    etag: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConditionalWriteReport {
    pub if_none_match: bool,
    pub if_match: bool,
    pub detail: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub store: Store,
    pub config: RepoConfig,
    cipher: Cipher,
}

fn blob_key(id: &str) -> String {
    format!("blobs/{}/{}", &id[..2.min(id.len())], id)
}
fn tool_blob_key(id: &str) -> String {
    format!("tools/blobs/{}/{}", &id[..2.min(id.len())], id)
}
fn head_key(state: &str) -> String {
    format!("refs/{state}/HEAD")
}
fn lease_key(state: &str) -> String {
    format!("refs/{state}/lease")
}
fn check_state(state: &str) -> Result<()> {
    let ok = !state.is_empty()
        && state.len() <= 128
        && state.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !state.starts_with('.');
    if ok { Ok(()) } else { Err(CarryError::Usage(format!("invalid state name '{state}'"))) }
}
fn check_id(id: &str) -> Result<()> {
    if id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(CarryError::Invalid(format!("malformed object id '{id}'")))
    }
}
fn json_err(e: serde_json::Error) -> CarryError {
    CarryError::Invalid(format!("json: {e}"))
}

impl Repo {
    /// Load `config`, or create it race-safely with If-None-Match.
    pub async fn open_or_init(
        store: Store,
        master_key: Option<MasterKey>,
        allow_no_encryption: bool,
    ) -> Result<Repo> {
        for _ in 0..3 {
            if let Some(raw) = store.get_opt(CONFIG_KEY).await? {
                return Self::from_config(store, &raw, master_key.as_ref());
            }
            let (cfg, cipher) = match &master_key {
                Some(mk) => {
                    let dk = DataKey::generate();
                    let cfg = RepoConfig {
                        format: FORMAT_VERSION,
                        encrypted: true,
                        cipher: "xchacha20poly1305+zstd".into(),
                        wrapped_keys: vec![dk.wrap(mk)],
                    };
                    (cfg, Cipher::Encrypted(dk))
                }
                None if allow_no_encryption => (
                    RepoConfig { format: FORMAT_VERSION, encrypted: false, cipher: "none+zstd".into(), wrapped_keys: vec![] },
                    Cipher::None,
                ),
                None => {
                    return Err(CarryError::BadKey(
                        "no master key: set CARRY_KEY or CARRY_KEY_FILE (or --no-encryption)".into(),
                    ))
                }
            };
            let body = serde_json::to_vec_pretty(&cfg).map_err(json_err)?;
            if store.put_if_absent(CONFIG_KEY, Bytes::from(body)).await? {
                return Ok(Repo { store, config: cfg, cipher });
            }
            // Lost the race: loop and load the winner's config.
        }
        Err(CarryError::Conflict("could not read or create repository config".into()))
    }

    /// Open an existing repo only; NotFound if no config.
    pub async fn open(store: Store, master_key: Option<MasterKey>) -> Result<Repo> {
        let raw = store
            .get_opt(CONFIG_KEY)
            .await?
            .ok_or_else(|| CarryError::NotFound("repository config (not initialised)".into()))?;
        Self::from_config(store, &raw, master_key.as_ref())
    }

    fn from_config(store: Store, raw: &[u8], mk: Option<&MasterKey>) -> Result<Repo> {
        let config: RepoConfig = serde_json::from_slice(raw).map_err(json_err)?;
        if config.format > FORMAT_VERSION {
            return Err(CarryError::Invalid(format!("repository format {} is newer than this carry", config.format)));
        }
        let cipher = if config.encrypted {
            let mk = mk.ok_or_else(|| CarryError::BadKey("repository is encrypted; set CARRY_KEY or CARRY_KEY_FILE".into()))?;
            Cipher::Encrypted(DataKey::unwrap_any(&config.wrapped_keys, mk)?)
        } else {
            Cipher::None
        };
        Ok(Repo { store, config, cipher })
    }

    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_encrypted()
    }

    /// Id a blob would get (without uploading).
    pub fn blob_id(&self, plaintext: &[u8]) -> String {
        self.cipher.blob_id(plaintext)
    }

    // ---- blobs ----

    /// Upload unless already present (dedup). Returns the id.
    pub async fn put_blob(&self, plaintext: &[u8]) -> Result<String> {
        self.put_blob_at(plaintext, blob_key).await
    }

    /// Like put_blob, also reporting whether an upload happened.
    pub async fn put_blob_report(&self, plaintext: &[u8]) -> Result<(String, bool)> {
        let id = self.cipher.blob_id(plaintext);
        let key = blob_key(&id);
        if self.store.exists(&key).await? {
            return Ok((id, false));
        }
        let body = self.cipher.encode_blob(plaintext)?;
        let uploaded = self.store.put_if_absent(&key, Bytes::from(body)).await?;
        Ok((id, uploaded))
    }

    async fn put_blob_at(&self, plaintext: &[u8], keyf: fn(&str) -> String) -> Result<String> {
        let id = self.cipher.blob_id(plaintext);
        let key = keyf(&id);
        if !self.store.exists(&key).await? {
            let body = self.cipher.encode_blob(plaintext)?;
            self.store.put_if_absent(&key, Bytes::from(body)).await?;
        }
        Ok(id)
    }

    /// Download, authenticate, decompress and verify the id.
    pub async fn get_blob(&self, id: &str) -> Result<Bytes> {
        self.get_blob_at(id, blob_key).await
    }

    pub async fn blob_exists(&self, id: &str) -> Result<bool> {
        check_id(id)?;
        self.store.exists(&blob_key(id)).await
    }

    async fn get_blob_at(&self, id: &str, keyf: fn(&str) -> String) -> Result<Bytes> {
        let mut out = Vec::new();
        self.get_to_at(id, keyf, &mut out).await?;
        Ok(Bytes::from(out))
    }

    /// Stream a blob's plaintext into `w` with bounded buffers (about one chunk).
    /// The id is verified at the end; on error `w` may hold partial data.
    pub async fn get_blob_to<W: std::io::Write + Send>(&self, id: &str, w: &mut W) -> Result<u64> {
        self.get_to_at(id, blob_key, w).await
    }

    pub async fn get_tool_blob_to<W: std::io::Write + Send>(&self, id: &str, w: &mut W) -> Result<u64> {
        self.get_to_at(id, tool_blob_key, w).await
    }

    /// Upload from a seekable reader of exactly `size` bytes with bounded buffers.
    /// Pass 1 hashes (the id), pass 2 uploads (multipart for chunked blobs).
    /// Returns (id, uploaded).
    pub async fn put_blob_from<R: std::io::Read + std::io::Seek + Send>(&self, r: R, size: u64) -> Result<(String, bool)> {
        self.put_from_at(r, size, blob_key).await
    }

    pub async fn put_tool_blob_from<R: std::io::Read + std::io::Seek + Send>(&self, r: R, size: u64) -> Result<(String, bool)> {
        self.put_from_at(r, size, tool_blob_key).await
    }

    async fn put_from_at<R: std::io::Read + std::io::Seek + Send>(
        &self,
        mut r: R,
        size: u64,
        keyf: fn(&str) -> String,
    ) -> Result<(String, bool)> {
        use std::io::SeekFrom;
        let ioe = |e: std::io::Error| CarryError::Internal(anyhow::anyhow!("read: {e}"));
        r.seek(SeekFrom::Start(0)).map_err(ioe)?;
        let mut h = self.cipher.id_hasher();
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut seen = 0u64;
        loop {
            let n = r.read(&mut buf).map_err(ioe)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            seen += n as u64;
        }
        if seen != size {
            return Err(CarryError::Invalid(format!("source changed while reading ({seen} != {size} bytes)")));
        }
        let id = h.finalize().to_hex().to_string();
        let key = keyf(&id);
        if self.store.exists(&key).await? {
            return Ok((id, false));
        }
        r.seek(SeekFrom::Start(0)).map_err(ioe)?;
        if size <= CHUNK_THRESHOLD {
            let mut pt = Vec::with_capacity(size as usize);
            std::io::Read::read_to_end(&mut r.take(size), &mut pt).map_err(ioe)?;
            if self.cipher.blob_id(&pt) != id {
                return Err(CarryError::Invalid("source changed while reading".into()));
            }
            let up = self.store.put_if_absent(&key, Bytes::from(self.cipher.encode_blob(&pt)?)).await?;
            return Ok((id, up));
        }
        let mut up = self.store.put_multipart(&key).await?;
        let res: Result<()> = async {
            let (hdr, mut enc) = self.cipher.chunk_encoder();
            let mut part = hdr;
            let mut h2 = self.cipher.id_hasher();
            let mut left = size;
            while left > 0 {
                let n = (left as usize).min(CHUNK_SIZE);
                r.read_exact(&mut buf[..n]).map_err(ioe)?;
                h2.update(&buf[..n]);
                left -= n as u64;
                part.extend_from_slice(&enc.frame(&buf[..n], left == 0)?);
                if part.len() >= PART_SIZE {
                    let p = std::mem::take(&mut part);
                    up.put_part(object_store::PutPayload::from(p)).await.map_err(crate::store::map_err)?;
                }
            }
            if h2.finalize().to_hex().as_str() != id {
                return Err(CarryError::Invalid("source changed while reading".into()));
            }
            if !part.is_empty() {
                up.put_part(object_store::PutPayload::from(part)).await.map_err(crate::store::map_err)?;
            }
            up.complete().await.map_err(crate::store::map_err)?;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            let _ = up.abort().await;
            return Err(e);
        }
        Ok((id, true))
    }

    async fn get_to_at<W: std::io::Write + Send>(&self, id: &str, keyf: fn(&str) -> String, w: &mut W) -> Result<u64> {
        use futures::StreamExt;
        check_id(id)?;
        let wr = |e: std::io::Error| CarryError::Internal(anyhow::anyhow!("write: {e}"));
        let mut s = self.store.get_stream(&keyf(id)).await?;
        let mut buf: Vec<u8> = Vec::new();
        // Read until we know the format.
        while buf.is_empty() {
            match s.next().await {
                Some(c) => buf.extend_from_slice(&c?),
                None => break,
            }
        }
        let mut h = self.cipher.id_hasher();
        let mut total = 0u64;
        if buf.first() == Some(&FMT_CHUNKED) {
            let plen = 1 + self.cipher.stream_prefix_len();
            let mut eof = false;
            // Fill until the header is present.
            while buf.len() < plen && !eof {
                match s.next().await {
                    Some(c) => buf.extend_from_slice(&c?),
                    None => eof = true,
                }
            }
            if buf.len() < plen {
                return Err(CarryError::Invalid(format!("blob {id}: truncated header")));
            }
            let mut dec = self.cipher.chunk_decoder(&buf[1..plen])?;
            let mut pos = plen;
            loop {
                // Need a frame header + payload in buf[pos..].
                let avail = buf.len() - pos;
                let need = if avail >= 4 {
                    let (len, _) = frame_header(buf[pos..pos + 4].try_into().unwrap())?;
                    4 + len
                } else {
                    4
                };
                if avail >= need && avail >= 4 {
                    let (len, last) = frame_header(buf[pos..pos + 4].try_into().unwrap())?;
                    let pt = dec.open_frame(&buf[pos + 4..pos + 4 + len], last)?;
                    h.update(&pt);
                    w.write_all(&pt).map_err(wr)?;
                    total += pt.len() as u64;
                    pos += 4 + len;
                    if dec.done {
                        // Drain: anything left is an error.
                        if pos != buf.len() {
                            return Err(CarryError::Invalid(format!("blob {id}: data after final chunk")));
                        }
                        while let Some(c) = s.next().await {
                            if !c?.is_empty() {
                                return Err(CarryError::Invalid(format!("blob {id}: data after final chunk")));
                            }
                        }
                        break;
                    }
                    continue;
                }
                // Compact, then read more.
                if pos > 0 {
                    buf.drain(..pos);
                    pos = 0;
                }
                match s.next().await {
                    Some(c) => buf.extend_from_slice(&c?),
                    None => return Err(CarryError::Invalid(format!("blob {id}: truncated (no final chunk)"))),
                }
            }
        } else {
            while let Some(c) = s.next().await {
                buf.extend_from_slice(&c?);
            }
            let pt = self.cipher.decode_single(&buf)?;
            h.update(&pt);
            w.write_all(&pt).map_err(wr)?;
            total = pt.len() as u64;
        }
        if h.finalize().to_hex().as_str() != id {
            return Err(CarryError::Invalid(format!("blob {id} content does not match its id")));
        }
        w.flush().map_err(wr)?;
        Ok(total)
    }

    // ---- manifests & HEAD ----

    pub async fn write_manifest(&self, m: &Manifest) -> Result<String> {
        let json = serde_json::to_vec(m).map_err(json_err)?;
        let id = self.cipher.blob_id(&json);
        let key = format!("manifests/{id}");
        if !self.store.exists(&key).await? {
            let body = self.cipher.compress_seal(&json)?;
            self.store.put_if_absent(&key, Bytes::from(body)).await?;
        }
        Ok(id)
    }

    pub async fn load_manifest(&self, id: &str) -> Result<Manifest> {
        check_id(id)?;
        let raw = self.store.get(&format!("manifests/{id}")).await?;
        let json = self.cipher.open_decompress(&raw)?;
        if self.cipher.blob_id(&json) != id {
            return Err(CarryError::Invalid(format!("manifest {id} does not match its id")));
        }
        serde_json::from_slice(&json).map_err(json_err)
    }

    /// Current HEAD and its etag (for `commit`).
    pub async fn read_head(&self, state: &str) -> Result<Option<(Head, String)>> {
        check_state(state)?;
        match self.store.get_with_etag(&head_key(state)).await? {
            None => Ok(None),
            Some((b, etag)) => Ok(Some((serde_json::from_slice(&b).map_err(json_err)?, etag))),
        }
    }

    /// Write manifest, then move HEAD by CAS. `expected_etag = None` means "state must not exist yet".
    /// Conflict if someone else moved HEAD.
    pub async fn commit(&self, state: &str, expected_etag: Option<String>, manifest: &Manifest) -> Result<Head> {
        check_state(state)?;
        let id = self.write_manifest(manifest).await?;
        let head = Head { manifest: id, seq: manifest.seq };
        let body = Bytes::from(serde_json::to_vec(&head).map_err(json_err)?);
        match expected_etag {
            None => {
                if !self.store.put_if_absent(&head_key(state), body).await? {
                    return Err(CarryError::Conflict(format!("state '{state}' was created concurrently")));
                }
            }
            Some(etag) => {
                self.store.put_if_match(&head_key(state), body, &etag).await.map_err(|e| match e {
                    CarryError::Conflict(_) => CarryError::Conflict(format!("state '{state}' moved concurrently")),
                    e => e,
                })?;
            }
        }
        Ok(head)
    }

    pub async fn list_states(&self) -> Result<Vec<String>> {
        let mut v: Vec<String> = self
            .store
            .list("refs/")
            .await?
            .into_iter()
            .filter_map(|k| k.strip_prefix("refs/").and_then(|r| r.strip_suffix("/HEAD")).map(str::to_string))
            .collect();
        v.sort();
        Ok(v)
    }

    // ---- lease ----

    /// Take `refs/<state>/lease` for `ttl_secs` (default 15 s). Conflict if another live holder has it.
    pub async fn acquire_lease(&self, state: &str, holder: &str, ttl_secs: Option<i64>) -> Result<Lease> {
        check_state(state)?;
        let key = lease_key(state);
        let expires = Utc::now() + chrono::Duration::seconds(ttl_secs.unwrap_or(LEASE_TTL_SECS));
        let body = Bytes::from(serde_json::to_vec(&LeaseRecord { holder: holder.into(), expires }).map_err(json_err)?);
        match self.store.get_with_etag(&key).await? {
            None => {
                if !self.store.put_if_absent(&key, body).await? {
                    return Err(CarryError::Conflict(format!("lease on '{state}' taken concurrently")));
                }
            }
            Some((b, etag)) => {
                let cur: LeaseRecord = serde_json::from_slice(&b).map_err(json_err)?;
                if cur.holder != holder && cur.expires > Utc::now() {
                    return Err(CarryError::Conflict(format!(
                        "state '{state}' is leased by {} until {}",
                        cur.holder, cur.expires
                    )));
                }
                self.store.put_if_match(&key, body, &etag).await?;
            }
        }
        let (_, etag) = self.store.get_with_etag(&key).await?.ok_or_else(|| CarryError::Conflict("lease vanished".into()))?;
        Ok(Lease { state: state.into(), holder: holder.into(), expires, etag })
    }

    /// Extend a held lease. Conflict if it was lost.
    pub async fn renew_lease(&self, lease: &mut Lease, ttl_secs: Option<i64>) -> Result<()> {
        let expires = Utc::now() + chrono::Duration::seconds(ttl_secs.unwrap_or(LEASE_TTL_SECS));
        let body = Bytes::from(
            serde_json::to_vec(&LeaseRecord { holder: lease.holder.clone(), expires }).map_err(json_err)?,
        );
        let etag = self.store.put_if_match(&lease_key(&lease.state), body, &lease.etag).await?;
        lease.etag = if etag.is_empty() {
            self.store.head(&lease_key(&lease.state)).await?.map(|h| h.1).unwrap_or_default()
        } else {
            etag
        };
        lease.expires = expires;
        Ok(())
    }

    /// Release if still ours (best effort; never deletes someone else's lease).
    pub async fn release_lease(&self, lease: Lease) -> Result<()> {
        let key = lease_key(&lease.state);
        if let Some((b, _)) = self.store.get_with_etag(&key).await? {
            let cur: LeaseRecord = serde_json::from_slice(&b).map_err(json_err)?;
            if cur.holder == lease.holder {
                self.store.delete(&key).await?;
            }
        }
        Ok(())
    }

    // ---- tool store ----

    /// Encrypted (same data key), deduplicated blob under tools/blobs/.
    pub async fn put_tool_blob(&self, plaintext: &[u8]) -> Result<String> {
        self.put_blob_at(plaintext, tool_blob_key).await
    }

    pub async fn get_tool_blob(&self, id: &str) -> Result<Bytes> {
        self.get_blob_at(id, tool_blob_key).await
    }

    /// Plaintext JSON at a `tools/...` key.
    pub async fn get_tool_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let key = tools_key(key)?;
        match self.store.get_opt(&key).await? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b).map(Some).map_err(json_err),
        }
    }

    pub async fn put_tool_json<T: Serialize>(&self, key: &str, v: &T) -> Result<()> {
        let key = tools_key(key)?;
        self.store.put(&key, Bytes::from(serde_json::to_vec_pretty(v).map_err(json_err)?)).await?;
        Ok(())
    }

    /// Ok(false) if the key already exists (e.g. an immutable version).
    pub async fn put_tool_json_if_absent<T: Serialize>(&self, key: &str, v: &T) -> Result<bool> {
        let key = tools_key(key)?;
        self.store.put_if_absent(&key, Bytes::from(serde_json::to_vec_pretty(v).map_err(json_err)?)).await
    }
}

fn tools_key(key: &str) -> Result<String> {
    let k = key.trim_start_matches('/');
    let k = if k.starts_with("tools/") { k.to_string() } else { format!("tools/{k}") };
    if k.split('/').any(|s| s == ".." || s.is_empty()) {
        return Err(CarryError::Usage(format!("bad tool key '{key}'")));
    }
    Ok(k)
}

/// Probe whether the store honours If-None-Match and If-Match on PUT.
pub async fn doctor_conditional_writes(store: &Store) -> Result<ConditionalWriteReport> {
    let key = format!("doctor/probe-{}", hex::encode(rand::random::<[u8; 8]>()));
    let mut detail = Vec::new();
    let mut if_none_match = false;
    let mut if_match = false;
    let res: Result<()> = async {
        if !store.put_if_absent(&key, Bytes::from_static(b"1")).await? {
            detail.push("fresh If-None-Match put refused".into());
            return Ok(());
        }
        if_none_match = !store.put_if_absent(&key, Bytes::from_static(b"2")).await?;
        if !if_none_match {
            detail.push("If-None-Match: * overwrote an existing object".into());
        }
        let (_, e1) = store.get_with_etag(&key).await?.ok_or_else(|| CarryError::Invalid("probe vanished".into()))?;
        let good = store.put_if_match(&key, Bytes::from_static(b"3"), &e1).await;
        let stale = store.put_if_match(&key, Bytes::from_static(b"4"), &e1).await;
        if_match = good.is_ok() && matches!(stale, Err(CarryError::Conflict(_)));
        if !if_match {
            detail.push(format!("If-Match: matching put ok={}, stale put conflict={}", good.is_ok(), matches!(stale, Err(CarryError::Conflict(_)))));
        }
        Ok(())
    }
    .await;
    let _ = store.delete(&key).await;
    res?;
    Ok(ConditionalWriteReport { if_none_match, if_match, detail })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Op;
    use std::sync::Arc;

    fn mem() -> Store {
        Store::from_object_store(Arc::new(object_store::memory::InMemory::new()), "r")
    }

    pub(crate) fn manifest(seq: u64, parent: Option<String>) -> Manifest {
        Manifest {
            format: FORMAT_VERSION,
            parent,
            seq,
            op: Op { kind: "init".into(), args: vec![], at: Utc::now(), by: "t".into() },
            entries: Default::default(),
            env: Default::default(),
            apps: Default::default(),
        }
    }

    #[tokio::test]
    async fn repo_in_memory() {
        let s = mem();
        let mk = MasterKey::generate();
        let r = Repo::open_or_init(s.clone(), Some(mk.clone()), false).await.unwrap();
        let id = r.put_blob(b"data").await.unwrap();
        assert_eq!(r.put_blob_report(b"data").await.unwrap(), (id.clone(), false));
        assert_eq!(&r.get_blob(&id).await.unwrap()[..], b"data");
        let h = r.commit("web-1", None, &manifest(1, None)).await.unwrap();
        assert!(matches!(r.commit("web-1", None, &manifest(1, None)).await, Err(CarryError::Conflict(_))));
        let (h2, etag) = r.read_head("web-1").await.unwrap().unwrap();
        assert_eq!(h, h2);
        r.commit("web-1", Some(etag.clone()), &manifest(2, Some(h.manifest.clone()))).await.unwrap();
        assert!(matches!(r.commit("web-1", Some(etag), &manifest(2, None)).await, Err(CarryError::Conflict(_))));
        assert_eq!(r.list_states().await.unwrap(), vec!["web-1"]);
        assert_eq!(r.load_manifest(&h.manifest).await.unwrap().seq, 1);

        let mut l = r.acquire_lease("web-1", "a", None).await.unwrap();
        assert!(matches!(r.acquire_lease("web-1", "b", None).await, Err(CarryError::Conflict(_))));
        r.renew_lease(&mut l, None).await.unwrap();
        r.release_lease(l).await.unwrap();
        r.acquire_lease("web-1", "b", None).await.unwrap();

        let t = r.put_tool_blob(b"bin").await.unwrap();
        assert_eq!(&r.get_tool_blob(&t).await.unwrap()[..], b"bin");

        assert!(matches!(Repo::open(s.clone(), Some(MasterKey::generate())).await, Err(CarryError::BadKey(_))));
        assert!(matches!(Repo::open(s, None).await, Err(CarryError::BadKey(_))));

        let rep = doctor_conditional_writes(&mem()).await.unwrap();
        assert!(rep.if_none_match && rep.if_match);
    }

    fn noise(n: usize) -> Vec<u8> {
        let mut v = vec![0u8; n];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut v);
        v
    }

    /// (offset, len) of each frame in a chunked object.
    fn frames(raw: &[u8], prefix: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut pos = 1 + prefix;
        while pos < raw.len() {
            let (len, _) = crate::crypto::frame_header(raw[pos..pos + 4].try_into().unwrap()).unwrap();
            out.push((pos, 4 + len));
            pos += 4 + len;
        }
        out
    }

    #[tokio::test]
    async fn chunked_blobs_stream_and_resist_truncation_and_reordering() {
        let s = mem();
        let r = Repo::open_or_init(s.clone(), Some(crate::crypto::MasterKey::from_bytes([7; 32])), false).await.unwrap();
        let data = noise(CHUNK_SIZE * 5 + 123);
        // Streaming put (multipart) and slice put agree on the id; dedup holds.
        let (id, up) = r.put_blob_from(std::io::Cursor::new(&data), data.len() as u64).await.unwrap();
        assert!(up);
        assert_eq!(r.put_blob_report(&data).await.unwrap(), (id.clone(), false));
        assert_eq!(id, r.blob_id(&data));
        let raw = s.get(&blob_key(&id)).await.unwrap().to_vec();
        assert_eq!(raw[0], FMT_CHUNKED);
        let mut out = Vec::new();
        assert_eq!(r.get_blob_to(&id, &mut out).await.unwrap(), data.len() as u64);
        assert!(out == data);
        // Size mismatch is refused.
        assert!(r.put_blob_from(std::io::Cursor::new(&data), 5).await.is_err());

        let fr = frames(&raw, crate::crypto::STREAM_PREFIX_LEN);
        assert_eq!(fr.len(), 6);
        let check = |body: Vec<u8>| {
            let (s, r, id) = (s.clone(), r.clone(), id.clone());
            async move {
                s.put(&blob_key(&id), Bytes::from(body)).await.unwrap();
                r.get_blob_to(&id, &mut Vec::new()).await
            }
        };
        // Truncated: drop the final frame.
        let (lo, _) = fr[5];
        assert!(matches!(check(raw[..lo].to_vec()).await, Err(CarryError::Invalid(_))));
        // Truncated at a non-final frame boundary, with the flag forged to "last".
        let (p4, l4) = fr[4];
        let mut forged = raw[..p4 + l4].to_vec();
        forged[p4] |= 0x80;
        assert!(matches!(check(forged).await, Err(CarryError::Invalid(_))));
        // Reordered: swap frames 1 and 2 (same length).
        let (a, la) = fr[1];
        let (b, lb) = fr[2];
        assert_eq!(la, lb);
        let mut sw = raw.clone();
        sw[a..a + la].copy_from_slice(&raw[b..b + lb]);
        sw[b..b + lb].copy_from_slice(&raw[a..a + la]);
        assert!(matches!(check(sw).await, Err(CarryError::Invalid(_))));
        // Trailing garbage after the final frame.
        let mut tail = raw.clone();
        tail.extend_from_slice(b"xx");
        assert!(matches!(check(tail).await, Err(CarryError::Invalid(_))));
        // Intact again.
        assert!(check(raw.clone()).await.is_ok());

        // Legacy headerless single-shot objects stay readable; new small ones carry FMT_SINGLE.
        let small = b"legacy blob".to_vec();
        let sid = r.blob_id(&small);
        s.put(&blob_key(&sid), Bytes::from(r.cipher.compress_seal(&small).unwrap())).await.unwrap();
        assert_eq!(&r.get_blob(&sid).await.unwrap()[..], &small[..]);
        let nid = r.put_blob(b"fresh").await.unwrap();
        assert_eq!(s.get(&blob_key(&nid)).await.unwrap()[0], crate::crypto::FMT_SINGLE);
    }

    #[tokio::test]
    async fn chunked_unencrypted_and_empty() {
        let r = Repo::open_or_init(mem(), None, true).await.unwrap();
        let data = noise(CHUNK_SIZE * 4 + 1);
        let (id, _) = r.put_blob_from(std::io::Cursor::new(&data), data.len() as u64).await.unwrap();
        assert!(r.get_blob(&id).await.unwrap() == data);
        let (e, _) = r.put_blob_from(std::io::Cursor::new(Vec::new()), 0).await.unwrap();
        assert!(r.get_blob(&e).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_key_needs_flag() {
        assert!(matches!(Repo::open_or_init(mem(), None, false).await, Err(CarryError::BadKey(_))));
        let r = Repo::open_or_init(mem(), None, true).await.unwrap();
        assert!(!r.is_encrypted());
    }
}

/// Live tests against MinIO/S3. Skipped unless CARRY_TEST_ENDPOINT is set; credentials come from
/// CARRY_TEST_ACCESS_KEY / CARRY_TEST_SECRET_KEY, bucket from CARRY_TEST_BUCKET (default "carry-test").
#[cfg(test)]
mod minio_tests {
    use super::tests::manifest;
    use super::*;
    use crate::store::StoreConfig;

    fn live(prefix: &str) -> Option<Store> {
        let endpoint = std::env::var("CARRY_TEST_ENDPOINT").ok()?;
        let cfg = StoreConfig {
            bucket: std::env::var("CARRY_TEST_BUCKET").unwrap_or_else(|_| "carry-test".into()),
            prefix: format!("{prefix}-{}", hex::encode(rand::random::<[u8; 6]>())),
            endpoint: Some(endpoint),
            region: Some("us-east-1".into()),
            access_key: std::env::var("CARRY_TEST_ACCESS_KEY").ok(),
            secret_key: std::env::var("CARRY_TEST_SECRET_KEY").ok(),
            allow_http: true,
        };
        Some(Store::new(&cfg).unwrap())
    }

    #[tokio::test]
    async fn minio_init_race_dedup_cas_tamper_wrongkey() {
        let Some(s) = live("core") else { return };
        let rep = doctor_conditional_writes(&s).await.unwrap();
        println!("conditional writes: {rep:?}");
        assert!(rep.if_none_match && rep.if_match, "{rep:?}");

        let mk = MasterKey::generate();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (s, mk) = (s.clone(), mk.clone());
                tokio::spawn(async move { Repo::open_or_init(s, Some(mk), false).await.unwrap() })
            })
            .collect();
        let mut repos = Vec::new();
        for t in tasks {
            repos.push(t.await.unwrap());
        }
        let first = repos[0].config.clone();
        assert!(repos.iter().all(|r| r.config == first));
        let r = &repos[0];
        let id = r.put_blob(b"x").await.unwrap();
        assert!(repos.iter().all(|o| o.blob_id(b"x") == id));

        let (_, uploaded) = repos[1].put_blob_report(b"x").await.unwrap();
        assert!(!uploaded);
        assert_eq!(s.list("blobs/").await.unwrap().len(), 1);

        let h = r.commit("web-1", None, &manifest(1, None)).await.unwrap();
        let (_, etag) = r.read_head("web-1").await.unwrap().unwrap();
        let (ma, mut mb) = (manifest(2, Some(h.manifest.clone())), manifest(2, Some(h.manifest.clone())));
        mb.op.by = "other".into();
        let a = r.commit("web-1", Some(etag.clone()), &ma);
        let b = repos[1].commit("web-1", Some(etag.clone()), &mb);
        let (a, b) = tokio::join!(a, b);
        assert!(a.is_ok() ^ b.is_ok(), "exactly one commit wins: {a:?} {b:?}");
        assert!(matches!(a.err().or(b.err()), Some(CarryError::Conflict(_))));

        let key = blob_key(&id);
        let mut raw = s.get(&key).await.unwrap().to_vec();
        let n = raw.len() - 1;
        raw[n] ^= 0x55;
        s.put(&key, Bytes::from(raw)).await.unwrap();
        assert!(matches!(r.get_blob(&id).await, Err(CarryError::Invalid(_))));

        let before = s.get(CONFIG_KEY).await.unwrap();
        assert!(matches!(
            Repo::open_or_init(s.clone(), Some(MasterKey::generate()), false).await,
            Err(CarryError::BadKey(_))
        ));
        assert_eq!(before, s.get(CONFIG_KEY).await.unwrap());

        for k in s.list("").await.unwrap() {
            s.delete(&k).await.unwrap();
        }
    }

    #[tokio::test]
    async fn minio_chunked_multipart_roundtrip() {
        let Some(s) = live("chunked") else { return };
        let r = Repo::open_or_init(s.clone(), None, true).await.unwrap();
        let mut data = vec![0u8; (CHUNK_SIZE * 19) + 77];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut data);
        let (id, up) = r.put_blob_from(std::io::Cursor::new(&data), data.len() as u64).await.unwrap();
        assert!(up);
        let mut out = Vec::new();
        r.get_blob_to(&id, &mut out).await.unwrap();
        assert!(out == data);
        for k in s.list("").await.unwrap() {
            s.delete(&k).await.unwrap();
        }
    }

    #[tokio::test]
    async fn minio_error_mapping() {
        let Some(_) = live("x") else { return };
        let cfg = StoreConfig {
            bucket: "carry-test".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            access_key: Some("a".into()),
            secret_key: Some("b".into()),
            allow_http: true,
            ..Default::default()
        };
        let e = Store::new(&cfg).unwrap().get("k").await.unwrap_err();
        assert_eq!(e.code(), "unreachable", "{e}");
        let mut cfg2 = cfg.clone();
        cfg2.endpoint = std::env::var("CARRY_TEST_ENDPOINT").ok();
        let e = Store::new(&cfg2).unwrap().get("k").await.unwrap_err();
        assert_eq!(e.code(), "refused", "{e}");
    }
}
