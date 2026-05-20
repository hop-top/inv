//! Blob storage facade.
//!
//! v1 ships a single backend — [`LocalBlobStore`] — that stores rendered
//! invoice PDFs (and any other byte payload) on the local filesystem.
//! The trait [`BlobStore`] is intentionally narrow: put/get/delete plus
//! a signed-URL helper for the `link://` delivery channel.
//!
//! The S3-compatible backend is gated behind the crate-level `s3` Cargo
//! feature and lands at v1.1 (real impl); the stub keeps the type graph
//! stable for downstream consumers.
//!
//! ## URI form
//!
//! Blob references are opaque strings shaped as
//! `blob://<backend>/<path>` — see [`BlobRef`].

pub mod blob_ref;
pub mod local;
pub mod store;

#[cfg(feature = "s3")]
pub mod s3;

pub use blob_ref::BlobRef;
pub use local::LocalBlobStore;
pub use store::BlobStore;
