//! [`BlobStore`] trait — the v1 facade.
//!
//! Mirrors the shape of kit's go `storage/blob.Store` (put / get / delete)
//! plus a v1-specific `sign_url` for the `link://` delivery channel
//! (signed shareable URL). `list` / `exists` are omitted at v1 — the
//! command layer holds the canonical inventory in `invoices.pdf_blob_ref`
//! and doesn't need backend introspection.

use std::time::Duration;

use async_trait::async_trait;

use super::BlobRef;
use crate::error::Result;

/// Asynchronous blob storage.
///
/// All backends MUST round-trip bytes via `put` / `get`. `sign_url` returns
/// a URL that the signed-link delivery channel can hand out (and that the
/// channel will later verify before serving the bytes). The semantics of
/// the signature are backend-defined — the local backend uses an HMAC-SHA256
/// token; S3-compat backends will use presigned URLs.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Write `content` to the store. `content_type` is hint metadata —
    /// some backends persist it, others ignore it. Returns the [`BlobRef`]
    /// the caller should persist on the owning row.
    async fn put(&self, content: Vec<u8>, content_type: &str) -> Result<BlobRef>;

    /// Read the bytes pointed to by `r`.
    async fn get(&self, r: &BlobRef) -> Result<Vec<u8>>;

    /// Delete the blob pointed to by `r`. Idempotent against
    /// already-missing entries (returns `Ok(())`).
    async fn delete(&self, r: &BlobRef) -> Result<()>;

    /// Produce a signed URL pointing at `r`, valid for `ttl`.
    ///
    /// The exact URL shape is backend-defined; the local backend returns
    /// a `file://...?exp=<unix>&sig=<hex>` form using HMAC-SHA256 over
    /// `<path>|<expiry>` keyed on the store's signing key.
    async fn sign_url(&self, r: &BlobRef, ttl: Duration) -> Result<String>;
}
