//! Keys, AEAD, keyed ids, signatures (ADR 0004).
//! See openspec/changes/carry-v0-1/design.md for this module's surface.
#![allow(dead_code)]

use crate::error::{CarryError, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const NONCE_LEN: usize = 24;
pub const ZSTD_LEVEL: i32 = 3;

/// A 256-bit master key. Never printed.
#[derive(Clone)]
pub struct MasterKey([u8; 32]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MasterKey({})", self.key_id())
    }
}

impl MasterKey {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        MasterKey(k)
    }
    pub fn from_bytes(b: [u8; 32]) -> Self {
        MasterKey(b)
    }
    pub fn from_base64(s: &str) -> Result<Self> {
        let raw = B64
            .decode(s.trim())
            .map_err(|_| CarryError::BadKey("master key is not valid base64".into()))?;
        let arr: [u8; 32] = raw
            .try_into()
            .map_err(|_| CarryError::BadKey("master key must be 32 bytes".into()))?;
        Ok(MasterKey(arr))
    }
    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let s = std::fs::read_to_string(path).map_err(|e| {
            CarryError::BadKey(format!("cannot read key file {}: {e}", path.display()))
        })?;
        Self::from_base64(&s)
    }
    /// `CARRY_KEY` (base64) or `CARRY_KEY_FILE`. Ok(None) when neither is set.
    pub fn from_env() -> Result<Option<Self>> {
        if let Ok(k) = std::env::var("CARRY_KEY") {
            if !k.trim().is_empty() {
                return Self::from_base64(&k).map(Some);
            }
        }
        if let Ok(p) = std::env::var("CARRY_KEY_FILE") {
            if !p.trim().is_empty() {
                return Self::from_file(std::path::Path::new(&p)).map(Some);
            }
        }
        Ok(None)
    }
    /// Non-secret identifier used to pick the matching wrapped key.
    pub fn key_id(&self) -> String {
        hex::encode(&blake3::derive_key("carry master key id v1", &self.0)[..8])
    }
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }
}

/// The repo data key, wrapped in `config` by one or more master keys.
#[derive(Clone)]
pub struct DataKey {
    key: [u8; 32],
    id_key: [u8; 32],
    /// Distinct id key for manifests (domain separation from blobs).
    manifest_id_key: [u8; 32],
}

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataKey(..)")
    }
}

/// One wrapped copy of the data key as stored in `config`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrappedKey {
    /// MasterKey::key_id of the wrapping key.
    pub kid: String,
    /// base64(nonce || ciphertext)
    pub data: String,
}

impl DataKey {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        Self::from_raw(k)
    }
    fn from_raw(key: [u8; 32]) -> Self {
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, &key);
        let mut id_key = [0u8; 32];
        hk.expand(b"carry blob id", &mut id_key)
            .expect("32 bytes is a valid hkdf length");
        let mut manifest_id_key = [0u8; 32];
        hk.expand(b"carry manifest id", &mut manifest_id_key)
            .expect("32 bytes is a valid hkdf length");
        DataKey {
            key,
            id_key,
            manifest_id_key,
        }
    }
    pub fn wrap(&self, mk: &MasterKey) -> WrappedKey {
        let sealed = seal_raw(&mk.cipher(), &self.key, b"carry data key v1");
        WrappedKey {
            kid: mk.key_id(),
            data: B64.encode(sealed),
        }
    }
    pub fn unwrap(w: &WrappedKey, mk: &MasterKey) -> Result<Self> {
        if w.kid != mk.key_id() {
            return Err(CarryError::BadKey(
                "master key does not match this repository".into(),
            ));
        }
        let raw = B64
            .decode(&w.data)
            .map_err(|_| CarryError::Invalid("wrapped key not base64".into()))?;
        let pt = open_raw(&mk.cipher(), &raw, b"carry data key v1")
            .map_err(|_| CarryError::BadKey("master key does not unwrap the data key".into()))?;
        let arr: [u8; 32] = pt
            .try_into()
            .map_err(|_| CarryError::Invalid("bad data key length".into()))?;
        Ok(Self::from_raw(arr))
    }
    /// Unwrap with whichever record matches `mk`.
    pub fn unwrap_any(ws: &[WrappedKey], mk: &MasterKey) -> Result<Self> {
        match ws.iter().find(|w| w.kid == mk.key_id()) {
            Some(w) => Self::unwrap(w, mk),
            None => Err(CarryError::BadKey(
                "master key does not match this repository".into(),
            )),
        }
    }
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.key).into())
    }
}

fn seal_raw(c: &XChaCha20Poly1305, pt: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = c
        .encrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload { msg: pt, aad },
        )
        .expect("xchacha encrypt cannot fail");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn open_raw(c: &XChaCha20Poly1305, data: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if data.len() < NONCE_LEN + 16 {
        return Err(CarryError::Invalid("ciphertext too short".into()));
    }
    let (n, ct) = data.split_at(NONCE_LEN);
    c.decrypt(
        XNonce::from_slice(n),
        chacha20poly1305::aead::Payload { msg: ct, aad },
    )
    .map_err(|_| CarryError::Invalid("authentication tag mismatch".into()))
}

/// Encryption mode of a repository, chosen by its `config`.
#[derive(Debug, Clone)]
pub enum Cipher {
    Encrypted(DataKey),
    /// `--no-encryption`: identity seal, plain BLAKE3 ids. Testing only.
    None,
}

impl Cipher {
    pub fn is_encrypted(&self) -> bool {
        matches!(self, Cipher::Encrypted(_))
    }
    /// nonce || ciphertext (identity for None).
    pub fn seal(&self, pt: &[u8]) -> Vec<u8> {
        match self {
            Cipher::Encrypted(k) => seal(k, pt),
            Cipher::None => pt.to_vec(),
        }
    }
    pub fn open(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            Cipher::Encrypted(k) => open(k, data),
            Cipher::None => Ok(data.to_vec()),
        }
    }
    pub fn compress_seal(&self, pt: &[u8]) -> Result<Vec<u8>> {
        Ok(self.seal(&compress(pt)?))
    }
    pub fn open_decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        decompress(&self.open(data)?)
    }
    pub fn blob_id(&self, pt: &[u8]) -> String {
        match self {
            Cipher::Encrypted(k) => blob_id(k, pt),
            Cipher::None => blake3::hash(pt).to_hex().to_string(),
        }
    }
}

// ---- manifests (domain-separated from blobs) ----

/// First byte of a v2 manifest object: id and AEAD bound to the "manifest" label and the state.
pub const FMT_MANIFEST_V2: u8 = 3;
/// First byte of a single-shot blob whose AEAD carries the "blob" label.
pub const FMT_SINGLE_V2: u8 = 4;
const BLOB_AAD_V2: &[u8] = b"carry blob v2";

fn manifest_aad(state: &str) -> Vec<u8> {
    let mut a = b"carry manifest v2\0".to_vec();
    a.extend_from_slice(state.as_bytes());
    a
}

impl Cipher {
    /// Keyed id for a manifest (label "carry manifest id"), distinct from any blob id.
    pub fn manifest_id(&self, json: &[u8]) -> String {
        match self {
            Cipher::Encrypted(k) => blake3::keyed_hash(&k.manifest_id_key, json)
                .to_hex()
                .to_string(),
            Cipher::None => {
                let mut h = blake3::Hasher::new_derive_key("carry manifest id v2");
                h.update(json);
                h.finalize().to_hex().to_string()
            }
        }
    }
    /// FMT_MANIFEST_V2 || seal(zstd(json), aad = "carry manifest v2" + state).
    pub fn seal_manifest(&self, json: &[u8], state: &str) -> Result<Vec<u8>> {
        let z = compress(json)?;
        let mut out = vec![FMT_MANIFEST_V2];
        match self {
            Cipher::Encrypted(k) => {
                out.extend_from_slice(&seal_raw(&k.cipher(), &z, &manifest_aad(state)))
            }
            Cipher::None => out.extend_from_slice(&z),
        }
        Ok(out)
    }
    /// Open a manifest object. Returns (json, is_v2). v2 objects must match `state`;
    /// legacy (pre-v2, unmarked) objects are still accepted with the legacy blob-domain id.
    pub fn open_manifest(&self, data: &[u8], state: &str) -> Result<(Vec<u8>, bool)> {
        if data.first() == Some(&FMT_MANIFEST_V2) {
            let r = match self {
                Cipher::Encrypted(k) => open_raw(&k.cipher(), &data[1..], &manifest_aad(state))
                    .and_then(|z| decompress(&z)),
                Cipher::None => decompress(&data[1..]),
            };
            if let Ok(j) = r {
                return Ok((j, true));
            }
        }
        // A legacy encrypted object starts with a random nonce byte, which may equal the marker.
        Ok((self.open_decompress(data)?, false))
    }
}

pub fn seal(k: &DataKey, pt: &[u8]) -> Vec<u8> {
    seal_raw(&k.cipher(), pt, b"")
}

/// Fails with `CarryError::Invalid` on a tag mismatch.
pub fn open(k: &DataKey, data: &[u8]) -> Result<Vec<u8>> {
    open_raw(&k.cipher(), data, b"")
}

pub fn compress(pt: &[u8]) -> Result<Vec<u8>> {
    zstd::encode_all(pt, ZSTD_LEVEL).map_err(|e| CarryError::Internal(anyhow::anyhow!("zstd: {e}")))
}

pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    zstd::decode_all(data).map_err(|e| CarryError::Invalid(format!("zstd: {e}")))
}

pub fn compress_seal(k: &DataKey, pt: &[u8]) -> Result<Vec<u8>> {
    Ok(seal(k, &compress(pt)?))
}

pub fn open_decompress(k: &DataKey, data: &[u8]) -> Result<Vec<u8>> {
    decompress(&open(k, data)?)
}

/// Hex keyed BLAKE3; key = HKDF-SHA256(data key, "carry blob id").
pub fn blob_id(k: &DataKey, pt: &[u8]) -> String {
    blake3::keyed_hash(&k.id_key, pt).to_hex().to_string()
}

// ---- blob envelope: single-shot or chunked STREAM (bounded memory) ----

/// First byte of every blob object written by this carry.
pub const FMT_SINGLE: u8 = 1;
pub const FMT_CHUNKED: u8 = 2;
/// Plaintext bytes per chunk in the chunked format.
pub const CHUNK_SIZE: usize = 1 << 20;
/// Blobs larger than this use the chunked format.
pub const CHUNK_THRESHOLD: u64 = 4 << 20;
/// XChaCha nonce (24) minus the STREAM counter (4) and last-flag (1).
pub const STREAM_PREFIX_LEN: usize = 19;
/// Frame header: u32 BE, top bit = final chunk, low 31 bits = payload length.
pub const FRAME_LAST: u32 = 1 << 31;
/// Largest payload a frame may declare (zstd can expand incompressible data slightly).
pub const FRAME_MAX: usize = CHUNK_SIZE + CHUNK_SIZE / 8 + 4096;
const STREAM_AAD: &[u8] = b"carry chunked blob v1";

type StreamEnc = chacha20poly1305::aead::stream::EncryptorBE32<XChaCha20Poly1305>;
type StreamDec = chacha20poly1305::aead::stream::DecryptorBE32<XChaCha20Poly1305>;

/// Seals plaintext chunks into frames. The STREAM construction authenticates the chunk
/// index and the final-chunk flag, so truncation, reordering and splicing fail to open.
pub struct ChunkEncoder {
    enc: Option<StreamEnc>,
    encrypted: bool,
    done: bool,
}

impl ChunkEncoder {
    /// One frame (header + payload). `last` must be true for exactly the final chunk.
    pub fn frame(&mut self, chunk: &[u8], last: bool) -> Result<Vec<u8>> {
        if self.done {
            return Err(CarryError::Internal(anyhow::anyhow!(
                "chunk after final chunk"
            )));
        }
        let z = compress(chunk)?;
        let payload = if self.encrypted {
            let p = chacha20poly1305::aead::Payload {
                msg: &z,
                aad: STREAM_AAD,
            };
            let r = if last {
                self.done = true;
                self.enc.take().expect("encoder present").encrypt_last(p)
            } else {
                self.enc.as_mut().expect("encoder present").encrypt_next(p)
            };
            r.map_err(|_| CarryError::Internal(anyhow::anyhow!("stream encrypt failed")))?
        } else {
            self.done = last;
            z
        };
        let mut hdr = payload.len() as u32;
        if last {
            hdr |= FRAME_LAST;
        }
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&hdr.to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }
}

/// Opens frames produced by `ChunkEncoder`, in order.
pub struct ChunkDecoder {
    dec: Option<StreamDec>,
    encrypted: bool,
    pub done: bool,
}

impl ChunkDecoder {
    pub fn open_frame(&mut self, payload: &[u8], last: bool) -> Result<Vec<u8>> {
        if self.done {
            return Err(CarryError::Invalid("data after final chunk".into()));
        }
        let z = if self.encrypted {
            let p = chacha20poly1305::aead::Payload {
                msg: payload,
                aad: STREAM_AAD,
            };
            let r = if last {
                self.done = true;
                self.dec.take().expect("decoder present").decrypt_last(p)
            } else {
                self.dec.as_mut().expect("decoder present").decrypt_next(p)
            };
            r.map_err(|_| CarryError::Invalid("chunk authentication failed".into()))?
        } else {
            self.done = last;
            payload.to_vec()
        };
        decompress(&z)
    }
}

/// Parse a frame header.
pub fn frame_header(h: [u8; 4]) -> Result<(usize, bool)> {
    let v = u32::from_be_bytes(h);
    let len = (v & !FRAME_LAST) as usize;
    if len > FRAME_MAX {
        return Err(CarryError::Invalid(format!(
            "chunk frame too large ({len} bytes)"
        )));
    }
    Ok((len, v & FRAME_LAST != 0))
}

impl Cipher {
    /// Streaming hasher producing the same id as `blob_id`.
    pub fn id_hasher(&self) -> blake3::Hasher {
        match self {
            Cipher::Encrypted(k) => blake3::Hasher::new_keyed(&k.id_key),
            Cipher::None => blake3::Hasher::new(),
        }
    }
    /// Length of the chunked-format header after the format byte.
    pub fn stream_prefix_len(&self) -> usize {
        if self.is_encrypted() {
            STREAM_PREFIX_LEN
        } else {
            0
        }
    }
    /// (header bytes incl. format byte, encoder).
    pub fn chunk_encoder(&self) -> (Vec<u8>, ChunkEncoder) {
        let mut hdr = vec![FMT_CHUNKED];
        match self {
            Cipher::Encrypted(k) => {
                let mut prefix = [0u8; STREAM_PREFIX_LEN];
                rand::thread_rng().fill_bytes(&mut prefix);
                hdr.extend_from_slice(&prefix);
                let enc = StreamEnc::from_aead(k.cipher(), (&prefix).into());
                (
                    hdr,
                    ChunkEncoder {
                        enc: Some(enc),
                        encrypted: true,
                        done: false,
                    },
                )
            }
            Cipher::None => (
                hdr,
                ChunkEncoder {
                    enc: None,
                    encrypted: false,
                    done: false,
                },
            ),
        }
    }
    /// Decoder for a chunked blob; `prefix` is the header after the format byte.
    pub fn chunk_decoder(&self, prefix: &[u8]) -> Result<ChunkDecoder> {
        match self {
            Cipher::Encrypted(k) => {
                let arr: [u8; STREAM_PREFIX_LEN] = prefix
                    .try_into()
                    .map_err(|_| CarryError::Invalid("bad chunked blob header".into()))?;
                let dec = StreamDec::from_aead(k.cipher(), (&arr).into());
                Ok(ChunkDecoder {
                    dec: Some(dec),
                    encrypted: true,
                    done: false,
                })
            }
            Cipher::None => Ok(ChunkDecoder {
                dec: None,
                encrypted: false,
                done: false,
            }),
        }
    }
    /// Whole blob object for an in-memory plaintext (single-shot or chunked by size).
    pub fn encode_blob(&self, pt: &[u8]) -> Result<Vec<u8>> {
        if pt.len() as u64 <= CHUNK_THRESHOLD {
            let z = compress(pt)?;
            let mut out = vec![FMT_SINGLE_V2];
            match self {
                Cipher::Encrypted(k) => {
                    out.extend_from_slice(&seal_raw(&k.cipher(), &z, BLOB_AAD_V2))
                }
                Cipher::None => out.extend_from_slice(&z),
            }
            return Ok(out);
        }
        let (mut out, mut enc) = self.chunk_encoder();
        let n = pt.len().div_ceil(CHUNK_SIZE);
        for (i, c) in pt.chunks(CHUNK_SIZE).enumerate() {
            out.extend_from_slice(&enc.frame(c, i + 1 == n)?);
        }
        Ok(out)
    }
    /// Decode a single-shot object (with FMT_SINGLE header, or a legacy headerless one).
    pub fn decode_single(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.first() == Some(&FMT_SINGLE_V2) {
            let r = match self {
                Cipher::Encrypted(k) => {
                    open_raw(&k.cipher(), &data[1..], BLOB_AAD_V2).and_then(|z| decompress(&z))
                }
                Cipher::None => decompress(&data[1..]),
            };
            if let Ok(v) = r {
                return Ok(v);
            }
        }
        if data.first() == Some(&FMT_SINGLE) {
            if let Ok(v) = self.open_decompress(&data[1..]) {
                return Ok(v);
            }
        }
        self.open_decompress(data)
    }
}

// ---- Ed25519 ----

/// Returns (secret key base64 [32-byte seed], public key base64).
pub fn ed25519_keygen() -> (String, String) {
    let sk = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    (
        B64.encode(sk.to_bytes()),
        B64.encode(sk.verifying_key().to_bytes()),
    )
}

pub fn ed25519_public(secret_b64: &str) -> Result<String> {
    Ok(B64.encode(signing_key(secret_b64)?.verifying_key().to_bytes()))
}

fn signing_key(secret_b64: &str) -> Result<ed25519_dalek::SigningKey> {
    let raw = B64
        .decode(secret_b64.trim())
        .map_err(|_| CarryError::BadKey("signing key not base64".into()))?;
    let arr: [u8; 32] = raw
        .try_into()
        .map_err(|_| CarryError::BadKey("signing key must be 32 bytes".into()))?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&arr))
}

/// base64 signature.
pub fn sign(secret_b64: &str, msg: &[u8]) -> Result<String> {
    use ed25519_dalek::Signer;
    Ok(B64.encode(signing_key(secret_b64)?.sign(msg).to_bytes()))
}

/// Err(Invalid) if the signature does not verify.
pub fn verify(public_b64: &str, msg: &[u8], sig_b64: &str) -> Result<()> {
    use ed25519_dalek::Verifier;
    let pk = B64
        .decode(public_b64.trim())
        .map_err(|_| CarryError::Invalid("public key not base64".into()))?;
    let pk: [u8; 32] = pk
        .try_into()
        .map_err(|_| CarryError::Invalid("public key must be 32 bytes".into()))?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk)
        .map_err(|_| CarryError::Invalid("bad public key".into()))?;
    let sig = B64
        .decode(sig_b64.trim())
        .map_err(|_| CarryError::Invalid("signature not base64".into()))?;
    let sig = ed25519_dalek::Signature::from_slice(&sig)
        .map_err(|_| CarryError::Invalid("bad signature".into()))?;
    vk.verify(msg, &sig)
        .map_err(|_| CarryError::Invalid("signature does not verify".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dk = DataKey::generate();
        let ct = compress_seal(&dk, b"hello world").unwrap();
        assert_eq!(open_decompress(&dk, &ct).unwrap(), b"hello world");
        assert_eq!(blob_id(&dk, b"x"), blob_id(&dk, b"x"));
        assert_ne!(blob_id(&dk, b"x"), blob_id(&DataKey::generate(), b"x"));
    }

    #[test]
    fn wrap_and_wrong_key() {
        let mk = MasterKey::generate();
        let mk2 = MasterKey::from_base64(&mk.to_base64()).unwrap();
        let dk = DataKey::generate();
        let w = dk.wrap(&mk);
        let back = DataKey::unwrap(&w, &mk2).unwrap();
        assert_eq!(blob_id(&dk, b"a"), blob_id(&back, b"a"));
        let other = MasterKey::generate();
        assert!(matches!(
            DataKey::unwrap(&w, &other),
            Err(CarryError::BadKey(_))
        ));
        // forged kid still fails at the AEAD layer
        let mut forged = w.clone();
        forged.kid = other.key_id();
        assert!(matches!(
            DataKey::unwrap(&forged, &other),
            Err(CarryError::BadKey(_))
        ));
    }

    #[test]
    fn tamper() {
        let dk = DataKey::generate();
        let mut ct = seal(&dk, b"secret");
        let last = ct.len() - 1;
        ct[last] ^= 1;
        assert!(matches!(open(&dk, &ct), Err(CarryError::Invalid(_))));
        assert!(matches!(
            open(&DataKey::generate(), &seal(&dk, b"s")),
            Err(CarryError::Invalid(_))
        ));
    }

    #[test]
    fn no_encryption_mode() {
        let c = Cipher::None;
        assert_eq!(c.seal(b"abc"), b"abc");
        assert_eq!(c.blob_id(b"abc"), blake3::hash(b"abc").to_hex().to_string());
    }

    #[test]
    fn manifest_and_blob_domains_are_separate() {
        let c = Cipher::Encrypted(DataKey::generate());
        let json = br#"{"a":1}"#;
        assert_ne!(c.manifest_id(json), c.blob_id(json));
        let m = c.seal_manifest(json, "web-1").unwrap();
        assert_eq!(c.open_manifest(&m, "web-1").unwrap(), (json.to_vec(), true));
        // Bound to the state and to the manifest label.
        assert!(c.open_manifest(&m, "web-2").is_err());
        assert!(c.decode_single(&m).is_err());
        let b = c.encode_blob(json).unwrap();
        assert_eq!(b[0], FMT_SINGLE_V2);
        assert_eq!(c.decode_single(&b).unwrap(), json);
        assert!(c.open_manifest(&b, "web-1").is_err());
        // Legacy objects still open.
        let legacy = c.compress_seal(json).unwrap();
        assert_eq!(
            c.open_manifest(&legacy, "web-1").unwrap(),
            (json.to_vec(), false)
        );
        let mut lb = vec![FMT_SINGLE];
        lb.extend_from_slice(&legacy);
        assert_eq!(c.decode_single(&lb).unwrap(), json);
    }

    #[test]
    fn signatures() {
        let (sk, pk) = ed25519_keygen();
        assert_eq!(ed25519_public(&sk).unwrap(), pk);
        let s = sign(&sk, b"m").unwrap();
        verify(&pk, b"m", &s).unwrap();
        assert!(verify(&pk, b"n", &s).is_err());
    }
}
