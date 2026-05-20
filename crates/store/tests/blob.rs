//! Integration tests for the blob facade.
//!
//! The local backend is unconditionally available — no Cargo feature
//! gates these tests beyond what `inv-store` already requires for its
//! sqlite default (which is irrelevant here; the blob module is
//! storage-backend-agnostic).

use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use inv_store::blob::{BlobRef, BlobStore, LocalBlobStore};

fn fresh_store() -> (TempDir, LocalBlobStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = LocalBlobStore::new(dir.path(), b"test-key").expect("new store");
    (dir, store)
}

#[tokio::test]
async fn put_then_get_round_trips_bytes() {
    let (_dir, store) = fresh_store();
    let payload = b"hello, invoice world".to_vec();
    let r = store
        .put(payload.clone(), "application/pdf")
        .await
        .expect("put");
    let got = store.get(&r).await.expect("get");
    assert_eq!(got, payload);
}

#[tokio::test]
async fn put_returns_blob_ref_with_local_backend() {
    let (_dir, store) = fresh_store();
    let r = store
        .put(b"x".to_vec(), "application/pdf")
        .await
        .expect("put");
    assert_eq!(r.backend(), "local");
    assert!(r.path().starts_with("blob_"), "path = {:?}", r.path());
    assert!(r.as_str().starts_with("blob://local/"));
}

#[tokio::test]
async fn delete_removes_the_file() {
    let (dir, store) = fresh_store();
    let r = store
        .put(b"goodbye".to_vec(), "application/pdf")
        .await
        .expect("put");
    let on_disk = dir.path().join(r.path());
    assert!(on_disk.exists(), "file should exist after put");

    store.delete(&r).await.expect("delete");
    assert!(!on_disk.exists(), "file should be gone after delete");

    // Second delete is a no-op (idempotent).
    store.delete(&r).await.expect("second delete is idempotent");
}

#[tokio::test]
async fn sign_url_returns_signed_file_uri() {
    let (_dir, store) = fresh_store();
    let r = store
        .put(b"signed".to_vec(), "application/pdf")
        .await
        .expect("put");
    let url = store
        .sign_url(&r, Duration::from_secs(60))
        .await
        .expect("sign");
    assert!(url.starts_with("file://"), "url = {url}");
    let (_, qs) = url.split_once('?').expect("query string");
    assert!(qs.contains("exp="), "missing expiry: {qs}");
    assert!(qs.contains("sig="), "missing signature: {qs}");

    // Different TTL → different expiry → different signature.
    let url2 = store
        .sign_url(&r, Duration::from_secs(120))
        .await
        .expect("sign 2");
    assert_ne!(url, url2);
}

#[test]
fn blob_ref_parse_format_round_trip() {
    let r = BlobRef::new("local", "blob_01J/abc").unwrap();
    assert_eq!(r.as_str(), "blob://local/blob_01J/abc");
    let parsed = BlobRef::from_str(r.as_str()).unwrap();
    assert_eq!(parsed, r);
    assert_eq!(parsed.backend(), "local");
    assert_eq!(parsed.path(), "blob_01J/abc");
}

#[test]
fn blob_ref_serde_string_form() {
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Holder {
        r: BlobRef,
    }
    let h = Holder {
        r: BlobRef::new("local", "blob_x").unwrap(),
    };
    let j = serde_json::to_string(&h).unwrap();
    assert_eq!(j, r#"{"r":"blob://local/blob_x"}"#);
    let back: Holder = serde_json::from_str(&j).unwrap();
    assert_eq!(back, h);
}

#[test]
fn blob_ref_rejects_non_blob_scheme() {
    assert!(BlobRef::parse("file:///etc/passwd").is_err());
    assert!(BlobRef::parse("blob://local").is_err()); // missing path
}
