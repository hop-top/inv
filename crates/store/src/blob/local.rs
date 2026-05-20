//! Filesystem-backed [`BlobStore`] implementation.
//!
//! Stores each blob as a single file under `<root>/<typeid-relative-path>`.
//! Keys are generated server-side at `put` time using `mti` (the same
//! TypeID facade used for domain ids), prefix `blob`. The returned
//! [`BlobRef`] looks like `blob://local/blob_01JABC...`.
//!
//! Signed URLs use HMAC-SHA256 keyed on the per-store `signing_key`.
//! Token shape (query string):
//!
//! ```text
//! file:///abs/path?exp=<unix-seconds>&sig=<hex(hmac-sha256(path|exp))>
//! ```
//!
//! Verification is the caller's job (the `link://` delivery channel does
//! it). We don't ship a verifier helper at v1 — the channel adapter
//! will own the validation step.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mti::prelude::*;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::store::BlobStore;
use super::BlobRef;
use crate::error::{Result, StoreError};

/// Local filesystem blob store.
///
/// Construct via [`LocalBlobStore::new`]. The store does not own the
/// signing key's secrecy — callers MUST treat the key like any other
/// secret (env var, KMS, …).
#[derive(Debug, Clone)]
pub struct LocalBlobStore {
    root: PathBuf,
    signing_key: Vec<u8>,
}

impl LocalBlobStore {
    /// Open a store rooted at `root`. The directory is created if
    /// missing. `signing_key` is used to HMAC signed URLs; pass a
    /// non-empty value in production.
    pub fn new(root: impl AsRef<Path>, signing_key: impl Into<Vec<u8>>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        // Canonicalise so resolve() can reject traversal attempts.
        let root = root.canonicalize()?;
        Ok(Self {
            root,
            signing_key: signing_key.into(),
        })
    }

    /// Backend tag the store advertises in [`BlobRef::backend`].
    pub const BACKEND: &'static str = "local";

    /// Resolve a [`BlobRef`] to an absolute path on disk, rejecting any
    /// attempt to escape the store root.
    fn resolve(&self, r: &BlobRef) -> Result<PathBuf> {
        if r.backend() != Self::BACKEND {
            return Err(StoreError::Blob(format!(
                "blob ref backend {:?} is not handled by local store",
                r.backend()
            )));
        }
        let candidate = self.root.join(r.path());
        // Defence in depth: lexical check (we can't canonicalize a not-yet-
        // existing file). Resolve any "." segments, reject ".." segments.
        for comp in Path::new(r.path()).components() {
            use std::path::Component;
            match comp {
                Component::Normal(_) | Component::CurDir => {}
                _ => {
                    return Err(StoreError::Blob(format!(
                        "blob path {:?} contains a disallowed component",
                        r.path()
                    )))
                }
            }
        }
        Ok(candidate)
    }

    /// Compute the HMAC-SHA256 hex digest of `msg` keyed on the store's
    /// signing key. Implemented inline (HMAC = H(k' ⊕ opad || H(k' ⊕ ipad || msg)))
    /// to avoid an extra crate for one call site.
    fn hmac_hex(&self, msg: &[u8]) -> String {
        const BLOCK: usize = 64; // SHA-256 block size
        let mut key = self.signing_key.clone();
        if key.len() > BLOCK {
            let mut h = Sha256::new();
            h.update(&key);
            key = h.finalize().to_vec();
        }
        if key.len() < BLOCK {
            key.resize(BLOCK, 0);
        }
        let mut ipad = [0u8; BLOCK];
        let mut opad = [0u8; BLOCK];
        for i in 0..BLOCK {
            ipad[i] = key[i] ^ 0x36;
            opad[i] = key[i] ^ 0x5c;
        }
        let mut inner = Sha256::new();
        inner.update(ipad);
        inner.update(msg);
        let inner_digest = inner.finalize();
        let mut outer = Sha256::new();
        outer.update(opad);
        outer.update(inner_digest);
        let final_digest = outer.finalize();
        hex_lower(&final_digest)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(ALPHABET[(b >> 4) as usize] as char);
        out.push(ALPHABET[(b & 0x0f) as usize] as char);
    }
    out
}

#[async_trait]
impl BlobStore for LocalBlobStore {
    async fn put(&self, content: Vec<u8>, _content_type: &str) -> Result<BlobRef> {
        // typeid-shaped, opaque-ish key — same id family as domain ids.
        let id = "blob".create_type_id::<V7>();
        let key = id.to_string();
        let target = self.root.join(&key);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&target, &content).await?;
        BlobRef::new(Self::BACKEND, &key)
    }

    async fn get(&self, r: &BlobRef) -> Result<Vec<u8>> {
        let path = self.resolve(r)?;
        Ok(fs::read(path).await?)
    }

    async fn delete(&self, r: &BlobRef) -> Result<()> {
        let path = self.resolve(r)?;
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    async fn sign_url(&self, r: &BlobRef, ttl: Duration) -> Result<String> {
        let path = self.resolve(r)?;
        let exp = SystemTime::now()
            .checked_add(ttl)
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .ok_or_else(|| StoreError::Blob("expiry overflowed system clock".into()))?
            .as_secs();
        let path_str = path.to_string_lossy();
        let msg = format!("{path_str}|{exp}");
        let sig = self.hmac_hex(msg.as_bytes());
        Ok(format!("file://{path_str}?exp={exp}&sig={sig}"))
    }
}
