//! S3 access via `object_store`. All keys are relative to the configured prefix.
//! See openspec/changes/carry-v0-1/design.md for this module's surface.
#![allow(dead_code)]

use crate::error::{CarryError, Result};
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut};
use object_store::path::Path;
use object_store::{
    ObjectStore, PutMode, PutOptions, PutPayload, RetryConfig, UpdateVersion,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Connection settings for one repository location.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreConfig {
    pub bucket: String,
    /// Key prefix without leading/trailing '/'. May be empty.
    pub prefix: String,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub allow_http: bool,
}

impl StoreConfig {
    /// Parse `s3://bucket/some/prefix`. Other fields are left default.
    pub fn from_repo_url(url: &str) -> Result<Self> {
        let rest = url
            .strip_prefix("s3://")
            .ok_or_else(|| CarryError::Usage(format!("repo url must start with s3://: {url}")))?;
        let (bucket, prefix) = match rest.split_once('/') {
            Some((b, p)) => (b, p.trim_matches('/')),
            None => (rest, ""),
        };
        if bucket.is_empty() {
            return Err(CarryError::Usage(format!("repo url has no bucket: {url}")));
        }
        let allow_http = false;
        Ok(StoreConfig {
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            allow_http,
            ..Default::default()
        })
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<dyn ObjectStore>,
    prefix: String,
    bucket: String,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Store(s3://{}/{})", self.bucket, self.prefix)
    }
}

/// Map an object_store error into the CLI error taxonomy.
pub(crate) fn map_err(e: object_store::Error) -> CarryError {
    use object_store::Error as E;
    match e {
        E::NotFound { path, .. } => CarryError::NotFound(path),
        E::AlreadyExists { path, .. } => CarryError::Conflict(format!("{path} already exists")),
        E::Precondition { path, .. } => CarryError::Conflict(format!("{path} changed concurrently")),
        E::NotSupported { source } => CarryError::Refused(format!("not supported: {source}")),
        other => classify(other.to_string()),
    }
}

fn classify(msg: String) -> CarryError {
    let m = msg.to_ascii_lowercase();
    let refused = [
        "403", "forbidden", "accessdenied", "access denied", "invalidaccesskeyid",
        "signaturedoesnotmatch", "401", "unauthorized", "nosuchbucket", "allaccessdisabled",
    ];
    let unreachable = [
        "error sending request", "connect", "dns", "timed out", "timeout", "connection",
        "broken pipe", "reset by peer", "unreachable",
    ];
    if refused.iter().any(|s| m.contains(s)) {
        CarryError::Refused(msg)
    } else if unreachable.iter().any(|s| m.contains(s)) {
        CarryError::Unreachable(msg)
    } else {
        CarryError::Internal(anyhow::anyhow!(msg))
    }
}

impl Store {
    pub fn new(cfg: &StoreConfig) -> Result<Self> {
        let mut b = AmazonS3Builder::from_env()
            .with_bucket_name(&cfg.bucket)
            .with_conditional_put(S3ConditionalPut::ETagMatch)
            .with_allow_http(cfg.allow_http)
            .with_retry(RetryConfig {
                max_retries: 3,
                retry_timeout: Duration::from_secs(20),
                ..Default::default()
            });
        if let Some(e) = &cfg.endpoint {
            b = b.with_endpoint(e);
            if e.starts_with("http://") {
                b = b.with_allow_http(true);
            }
        }
        b = b.with_region(cfg.region.clone().unwrap_or_else(|| {
            std::env::var("AWS_REGION")
                .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                .unwrap_or_else(|_| "us-east-1".into())
        }));
        if let Some(k) = &cfg.access_key {
            b = b.with_access_key_id(k);
        }
        if let Some(s) = &cfg.secret_key {
            b = b.with_secret_access_key(s);
        }
        let s3: AmazonS3 = b
            .build()
            .map_err(|e| CarryError::Usage(format!("bad store settings: {e}")))?;
        Ok(Store {
            inner: Arc::new(s3),
            prefix: cfg.prefix.trim_matches('/').to_string(),
            bucket: cfg.bucket.clone(),
        })
    }

    /// Alias kept for the design doc surface.
    pub fn from_settings(cfg: &StoreConfig) -> Result<Self> {
        Self::new(cfg)
    }

    /// Wrap any ObjectStore (used in tests with InMemory).
    pub fn from_object_store(inner: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        Store { inner, prefix: prefix.trim_matches('/').to_string(), bucket: "custom".into() }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn path(&self, key: &str) -> Path {
        let key = key.trim_start_matches('/');
        if self.prefix.is_empty() {
            Path::from(key)
        } else {
            Path::from(format!("{}/{}", self.prefix, key))
        }
    }

    pub async fn get(&self, key: &str) -> Result<Bytes> {
        let r = self.inner.get(&self.path(key)).await.map_err(map_err)?;
        r.bytes().await.map_err(map_err)
    }

    pub async fn get_opt(&self, key: &str) -> Result<Option<Bytes>> {
        match self.get(key).await {
            Ok(b) => Ok(Some(b)),
            Err(CarryError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn get_with_etag(&self, key: &str) -> Result<Option<(Bytes, String)>> {
        let r = match self.inner.get(&self.path(key)).await {
            Ok(r) => r,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(map_err(e)),
        };
        let etag = r.meta.e_tag.clone().unwrap_or_default();
        let b = r.bytes().await.map_err(map_err)?;
        Ok(Some((b, etag)))
    }

    /// Stream an object into `w` with bounded memory. Returns bytes written.
    pub async fn get_to<W: AsyncWrite + Unpin>(&self, key: &str, w: &mut W) -> Result<u64> {
        let r = self.inner.get(&self.path(key)).await.map_err(map_err)?;
        let mut s = r.into_stream();
        let mut n = 0u64;
        while let Some(chunk) = s.next().await {
            let chunk = chunk.map_err(map_err)?;
            n += chunk.len() as u64;
            w.write_all(&chunk)
                .await
                .map_err(|e| CarryError::Internal(anyhow::anyhow!("write: {e}")))?;
        }
        w.flush().await.map_err(|e| CarryError::Internal(anyhow::anyhow!("flush: {e}")))?;
        Ok(n)
    }

    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<String> {
        let r = self
            .inner
            .put(&self.path(key), PutPayload::from_bytes(bytes))
            .await
            .map_err(map_err)?;
        Ok(r.e_tag.unwrap_or_default())
    }

    /// `If-None-Match: *`. Ok(false) if the key already exists.
    pub async fn put_if_absent(&self, key: &str, bytes: Bytes) -> Result<bool> {
        let opts = PutOptions { mode: PutMode::Create, ..Default::default() };
        match self.inner.put_opts(&self.path(key), PutPayload::from_bytes(bytes), opts).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. })
            | Err(object_store::Error::Precondition { .. }) => Ok(false),
            Err(e) => Err(map_err(e)),
        }
    }

    /// `If-Match: etag`. Returns the new etag; precondition failure -> Conflict.
    pub async fn put_if_match(&self, key: &str, bytes: Bytes, etag: &str) -> Result<String> {
        let opts = PutOptions {
            mode: PutMode::Update(UpdateVersion { e_tag: Some(etag.to_string()), version: None }),
            ..Default::default()
        };
        match self.inner.put_opts(&self.path(key), PutPayload::from_bytes(bytes), opts).await {
            Ok(r) => Ok(r.e_tag.unwrap_or_default()),
            Err(object_store::Error::Precondition { path, .. })
            | Err(object_store::Error::NotFound { path, .. }) => {
                Err(CarryError::Conflict(format!("{path} changed concurrently")))
            }
            Err(e) => Err(map_err(e)),
        }
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.inner.delete(&self.path(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        match self.inner.head(&self.path(key)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Size and etag of an object, if present.
    pub async fn head(&self, key: &str) -> Result<Option<(u64, String)>> {
        match self.inner.head(&self.path(key)).await {
            Ok(m) => Ok(Some((m.size as u64, m.e_tag.unwrap_or_default()))),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Recursive listing; returned keys are relative to the store prefix.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let p = self.path(prefix.trim_end_matches('/'));
        let p = if prefix.trim_matches('/').is_empty() && self.prefix.is_empty() {
            None
        } else {
            Some(p)
        };
        let metas: Vec<_> = self.inner.list(p.as_ref()).try_collect().await.map_err(map_err)?;
        let strip = if self.prefix.is_empty() { String::new() } else { format!("{}/", self.prefix) };
        let mut out: Vec<String> = metas
            .into_iter()
            .map(|m| m.location.to_string())
            .map(|s| s.strip_prefix(&strip).map(str::to_string).unwrap_or(s))
            .collect();
        out.sort();
        Ok(out)
    }

    /// Immediate "subdirectories" under `prefix` (relative names, no trailing '/').
    pub async fn list_dirs(&self, prefix: &str) -> Result<Vec<String>> {
        let p = self.path(prefix.trim_end_matches('/'));
        let r = self.inner.list_with_delimiter(Some(&p)).await.map_err(map_err)?;
        let mut out: Vec<String> = r
            .common_prefixes
            .into_iter()
            .filter_map(|c| c.filename().map(str::to_string))
            .collect();
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url() {
        let c = StoreConfig::from_repo_url("s3://b/a/b/").unwrap();
        assert_eq!((c.bucket.as_str(), c.prefix.as_str()), ("b", "a/b"));
        let c = StoreConfig::from_repo_url("s3://b").unwrap();
        assert_eq!(c.prefix, "");
        assert!(StoreConfig::from_repo_url("http://x").is_err());
    }

    #[test]
    fn classify_errors() {
        assert_eq!(classify("HTTP 403 Forbidden".into()).code(), "refused");
        assert_eq!(classify("error sending request for url".into()).code(), "unreachable");
    }

    #[tokio::test]
    async fn in_memory_conditional() {
        let s = Store::from_object_store(Arc::new(object_store::memory::InMemory::new()), "p");
        assert!(s.put_if_absent("k", Bytes::from_static(b"1")).await.unwrap());
        assert!(!s.put_if_absent("k", Bytes::from_static(b"2")).await.unwrap());
        let (_, e) = s.get_with_etag("k").await.unwrap().unwrap();
        let e2 = s.put_if_match("k", Bytes::from_static(b"3"), &e).await.unwrap();
        assert!(matches!(
            s.put_if_match("k", Bytes::from_static(b"4"), &e).await,
            Err(CarryError::Conflict(_))
        ));
        assert_ne!(e, e2);
        assert_eq!(s.list("").await.unwrap(), vec!["k".to_string()]);
        let mut v = Vec::new();
        s.get_to("k", &mut v).await.unwrap();
        assert_eq!(v, b"3");
    }
}
