//! HMAC-SHA256 signed tokens for the public `/v/{token}` view route.
//!
//! Token shape: `base64url( invoice_id || ":" || expiry_unix || ":" || sig )`
//! where `sig = hex( HMAC-SHA256(invoice_id || ":" || expiry_unix, signing_key) )`.
//!
//! Encoding choices:
//!
//! - base64url with no padding so tokens are URL-safe out of the box.
//! - The HMAC digest is rendered in lowercase hex to keep the inner
//!   payload printable; the outer base64url wrap is what gives us the
//!   compact URL form.
//! - Invoice ids are typeid-shaped (e.g. `invoice_01J...`) and never
//!   contain `:`, so we can use it as the inner separator without
//!   escaping.
//!
//! Verification rejects: malformed base64, malformed inner payload,
//! expired tokens (`expiry <= now`), and signature mismatch — all three
//! collapse to [`crate::error::ApiError::InvalidLink`] (404) so the
//! public route never leaks why a token was rejected.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Sign an `invoice_id` for `ttl_secs` seconds and return the wire token.
pub fn sign(invoice_id: &str, ttl_secs: u64, signing_key: &[u8]) -> String {
    let exp = current_unix_seconds() + ttl_secs;
    let payload = format!("{invoice_id}:{exp}");
    let sig = hmac_sha256_hex(signing_key, payload.as_bytes());
    let composed = format!("{payload}:{sig}");
    URL_SAFE_NO_PAD.encode(composed.as_bytes())
}

/// Verified payload of a signed-link token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken {
    /// Invoice id embedded in the token (string form; caller parses to
    /// the typed [`inv_core::domain::ids::InvoiceId`]).
    pub invoice_id: String,
    /// Unix-seconds expiry encoded in the token.
    pub expiry_unix: u64,
}

/// Errors raised when verifying a signed-link token. Collapsed into a
/// single 404 by the public route — exposed here for testing.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    /// Token did not decode as base64url.
    #[error("malformed base64")]
    Base64,
    /// Decoded payload did not contain `invoice_id:exp:sig`.
    #[error("malformed payload")]
    Payload,
    /// Expiry was past the supplied `now`.
    #[error("expired")]
    Expired,
    /// Signature did not match.
    #[error("signature mismatch")]
    Mismatch,
}

/// Verify a token against the supplied signing key + current time.
pub fn verify(token: &str, signing_key: &[u8]) -> Result<VerifiedToken, VerifyError> {
    verify_at(token, signing_key, current_unix_seconds())
}

/// Same as [`verify`] but with an injectable `now` for tests.
pub fn verify_at(
    token: &str,
    signing_key: &[u8],
    now_unix: u64,
) -> Result<VerifiedToken, VerifyError> {
    let raw = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| VerifyError::Base64)?;
    let composed = std::str::from_utf8(&raw).map_err(|_| VerifyError::Payload)?;
    let mut parts = composed.rsplitn(2, ':');
    let sig = parts.next().ok_or(VerifyError::Payload)?;
    let payload = parts.next().ok_or(VerifyError::Payload)?;

    let expected_sig = hmac_sha256_hex(signing_key, payload.as_bytes());
    if !constant_time_eq(sig.as_bytes(), expected_sig.as_bytes()) {
        return Err(VerifyError::Mismatch);
    }

    let (invoice_id, exp_s) = payload.split_once(':').ok_or(VerifyError::Payload)?;
    let expiry_unix: u64 = exp_s.parse().map_err(|_| VerifyError::Payload)?;
    if expiry_unix <= now_unix {
        return Err(VerifyError::Expired);
    }
    Ok(VerifiedToken {
        invoice_id: invoice_id.to_string(),
        expiry_unix,
    })
}

/// HMAC-SHA256 as lowercase hex. Inline impl avoids pulling in the `hmac`
/// crate for this single call site — `inv-store::blob::local` does the
/// same.
fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    const BLOCK: usize = 64; // SHA-256 block size
    let mut k = key.to_vec();
    if k.len() > BLOCK {
        let mut h = Sha256::new();
        h.update(&k);
        k = h.finalize().to_vec();
    }
    if k.len() < BLOCK {
        k.resize(BLOCK, 0);
    }
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    hex_lower(&outer.finalize())
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let key = b"some-key";
        let t = sign("invoice_01abc", 600, key);
        let v = verify(&t, key).expect("verifies");
        assert_eq!(v.invoice_id, "invoice_01abc");
    }

    #[test]
    fn expired_rejected() {
        let key = b"some-key";
        let t = sign("invoice_01abc", 600, key);
        let v = verify_at(&t, key, current_unix_seconds() + 3600);
        assert_eq!(v, Err(VerifyError::Expired));
    }

    #[test]
    fn tampered_signature_rejected() {
        let key = b"some-key";
        let t = sign("invoice_01abc", 600, key);
        // Flip the last byte (a base64url char inside the encoded body).
        let mut bytes = t.into_bytes();
        let last = bytes.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        // Could be Mismatch or Base64/Payload depending on which byte
        // flipped — any rejection is acceptable here.
        assert!(verify(&tampered, key).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let t = sign("invoice_01abc", 600, b"key-a");
        assert_eq!(verify(&t, b"key-b"), Err(VerifyError::Mismatch));
    }
}
