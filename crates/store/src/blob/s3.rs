//! S3-compatible blob store — STUB.
//!
//! Gated by the `s3` Cargo feature. The real implementation lands at
//! v1.1; this stub exists so downstream crates can compile-check the
//! eventual surface (`Arc<dyn BlobStore>` factories, config schema,
//! etc.) without pulling in the AWS SDK pre-emptively.
//!
//! All methods `todo!()` at runtime — callers reaching this code is
//! a configuration bug, not a runtime path.

use std::time::Duration;

use async_trait::async_trait;

use super::store::BlobStore;
use super::BlobRef;
use crate::error::Result;

/// Stub S3 blob store. Real impl at v1.1.
#[derive(Debug, Clone, Default)]
pub struct S3BlobStore {
    /// Bucket name (placeholder — populated by the real impl).
    pub bucket: String,
    /// Optional path prefix inside the bucket.
    pub prefix: String,
}

impl S3BlobStore {
    /// Backend tag this store advertises in [`BlobRef::backend`].
    pub const BACKEND: &'static str = "s3";

    /// Constructor placeholder. Real impl will accept an AWS config.
    pub fn new(bucket: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: prefix.into(),
        }
    }
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, _content: Vec<u8>, _content_type: &str) -> Result<BlobRef> {
        todo!("s3 backend lands at inv v1.1 — track: hop-top/inv#s3-blob")
    }

    async fn get(&self, _r: &BlobRef) -> Result<Vec<u8>> {
        todo!("s3 backend lands at inv v1.1 — track: hop-top/inv#s3-blob")
    }

    async fn delete(&self, _r: &BlobRef) -> Result<()> {
        todo!("s3 backend lands at inv v1.1 — track: hop-top/inv#s3-blob")
    }

    async fn sign_url(&self, _r: &BlobRef, _ttl: Duration) -> Result<String> {
        todo!("s3 backend lands at inv v1.1 — track: hop-top/inv#s3-blob")
    }
}
