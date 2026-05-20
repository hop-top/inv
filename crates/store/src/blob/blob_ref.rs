//! [`BlobRef`] — opaque pointer to a stored blob.
//!
//! Wire form: `blob://<backend>/<path>`. The `backend` segment selects
//! which [`super::BlobStore`] impl owns the bytes (`local`, `s3`, …);
//! `path` is backend-defined (typeid-shaped relative path for the local
//! backend).
//!
//! Stored as a `TEXT` column on `invoices.pdf_blob_ref` (see design
//! spec §5).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::StoreError;

/// Opaque reference to a stored blob.
///
/// Construct via [`BlobRef::new`] / [`BlobRef::parse`] / `FromStr` rather
/// than poking the inner string directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobRef(String);

impl BlobRef {
    /// Build a `blob://<backend>/<path>` reference from parts.
    ///
    /// `backend` is normalised to lower-case; `path` is stored verbatim
    /// (any leading `/` is stripped). Neither part is checked beyond
    /// "non-empty backend".
    pub fn new(backend: &str, path: &str) -> Result<Self, StoreError> {
        if backend.is_empty() {
            return Err(StoreError::Blob("empty backend segment".to_string()));
        }
        let path = path.strip_prefix('/').unwrap_or(path);
        Ok(Self(format!(
            "blob://{}/{}",
            backend.to_ascii_lowercase(),
            path
        )))
    }

    /// Parse a `blob://...` URI into a [`BlobRef`].
    pub fn parse(s: &str) -> Result<Self, StoreError> {
        let rest = s
            .strip_prefix("blob://")
            .ok_or_else(|| StoreError::Blob(format!("missing blob:// scheme in {s:?}")))?;
        let (backend, path) = rest
            .split_once('/')
            .ok_or_else(|| StoreError::Blob(format!("missing path segment in {s:?}")))?;
        Self::new(backend, path)
    }

    /// The backend segment (`local`, `s3`, …).
    pub fn backend(&self) -> &str {
        // safe: invariants enforced by constructors.
        let rest = self.0.strip_prefix("blob://").unwrap_or(&self.0);
        rest.split_once('/').map(|(b, _)| b).unwrap_or(rest)
    }

    /// Backend-defined path portion (does not include the leading `/`).
    pub fn path(&self) -> &str {
        let rest = self.0.strip_prefix("blob://").unwrap_or(&self.0);
        rest.split_once('/').map(|(_, p)| p).unwrap_or("")
    }

    /// Full URI form (`blob://<backend>/<path>`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BlobRef {
    type Err = StoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_parse_format() {
        let r = BlobRef::new("local", "invoices/inv_01J.pdf").unwrap();
        assert_eq!(r.as_str(), "blob://local/invoices/inv_01J.pdf");
        assert_eq!(r.backend(), "local");
        assert_eq!(r.path(), "invoices/inv_01J.pdf");

        let parsed: BlobRef = r.as_str().parse().unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn rejects_missing_scheme() {
        assert!(BlobRef::parse("file:///nope").is_err());
        assert!(BlobRef::parse("local/path").is_err());
    }

    #[test]
    fn rejects_missing_path_segment() {
        // "blob://local" has no path slash.
        assert!(BlobRef::parse("blob://local").is_err());
    }

    #[test]
    fn rejects_empty_backend() {
        assert!(BlobRef::new("", "x").is_err());
    }

    #[test]
    fn backend_normalised_to_lowercase() {
        let r = BlobRef::new("LOCAL", "x").unwrap();
        assert_eq!(r.backend(), "local");
    }

    #[test]
    fn strips_leading_slash_on_path() {
        let r = BlobRef::new("local", "/foo/bar").unwrap();
        assert_eq!(r.path(), "foo/bar");
        assert_eq!(r.as_str(), "blob://local/foo/bar");
    }
}
