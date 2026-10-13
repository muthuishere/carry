//! Shared types. Changing these is a cross-team change: coordinate first.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;
/// Files smaller than this are stored inline in the manifest.
pub const INLINE_MAX: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Head {
    pub manifest: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    /// Unix permission bits, e.g. 0o644.
    pub mode: u32,
    pub size: u64,
    /// Blob id when stored as a blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// Base64 content when stored inline (size < INLINE_MAX).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    /// Symlink target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Op {
    /// put | rm | mv | env_set | env_unset | env_import | rollback | tool_add | tool_rm | init
    pub kind: String,
    pub args: Vec<String>,
    pub at: chrono::DateTime<chrono::Utc>,
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default)]
    pub prefetch: bool,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    pub parent: Option<String>,
    pub seq: u64,
    pub op: Op,
    /// Absolute container path -> entry. Keys always start with '/'.
    pub entries: BTreeMap<String, Entry>,
    pub env: BTreeMap<String, String>,
    /// apps.lock: tool name -> reference.
    pub apps: BTreeMap<String, ToolRef>,
}

/// Signed tool metadata stored at tools/<name>/<version>/tool.json
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolMeta {
    pub name: String,
    pub version: String,
    /// "linux/amd64" -> platform build
    pub platforms: BTreeMap<String, ToolBuild>,
    #[serde(default)]
    pub yanked: bool,
    pub published_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolBuild {
    /// sha256 of the plaintext binary
    pub sha256: String,
    pub size: u64,
    /// blob id in the tool store
    pub blob: String,
}

/// Envelope stored at tools/<name>/<version>/tool.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedToolMeta {
    pub meta: ToolMeta,
    /// base64 Ed25519 signature over serde_json::to_vec(&meta)
    pub signature: String,
    /// base64 public key of the signer
    pub signer: String,
}
