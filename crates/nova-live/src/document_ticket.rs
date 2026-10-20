//! Short-lived, person-bound authority for persistent document rooms.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Claims {
    pub purpose: String,
    pub origin: String,
    pub document_id: String,
    pub organization_id: String,
    pub app_id: String,
    pub user_id: String,
    pub name: String,
    pub role: String,
    pub issued_at: u64,
    pub expires_at: u64,
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
pub fn decode(payload: &str) -> Option<Claims> {
    if payload.len() > 16384 {
        return None;
    }
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}
pub fn verify(payload: &str, sig: &str, key: &[u8], origin: &str, now: u64) -> Option<Claims> {
    if sig.len() > 128 {
        return None;
    }
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(payload.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).ok()?)
        .ok()?;
    let c = decode(payload)?;
    if c.purpose != "native-document-session"
        || c.origin != origin
        || c.app_id != "novaphoto"
        || c.issued_at > now
        || c.expires_at <= now
        || c.expires_at.checked_sub(c.issued_at)? > 300
        || !matches!(c.role.as_str(), "owner" | "editor" | "viewer")
        || crate::documents::valid_id(&c.document_id).is_err()
        || crate::documents::valid_id(&c.organization_id).is_err()
        || crate::documents::valid_id(&c.user_id).is_err()
        || c.name.trim().is_empty()
        || c.name.chars().count() > 160
    {
        return None;
    }
    Some(c)
}
pub fn key_from_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect()
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    #[test]
    fn tickets_are_signed_timed_and_bound_to_the_document_origin() {
        let key = Ed25519KeyPair::from_seed_unchecked(&[8; 32]).unwrap();
        let c = Claims {
            purpose: "native-document-session".into(),
            origin: "https://example.invalid".into(),
            document_id: "00000000-0000-4000-8000-000000000001".into(),
            organization_id: "00000000-0000-4000-8000-000000000002".into(),
            app_id: "novaphoto".into(),
            user_id: "00000000-0000-4000-8000-000000000003".into(),
            name: "Test".into(),
            role: "viewer".into(),
            issued_at: 100,
            expires_at: 400,
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).unwrap());
        let sig = URL_SAFE_NO_PAD.encode(key.sign(payload.as_bytes()).as_ref());
        assert!(
            verify(
                &payload,
                &sig,
                key.public_key().as_ref(),
                "https://example.invalid",
                101
            )
            .is_some()
        );
        assert!(
            verify(
                &payload,
                &sig,
                key.public_key().as_ref(),
                "https://other.invalid",
                101
            )
            .is_none()
        );
        assert!(
            verify(
                &payload,
                &sig,
                key.public_key().as_ref(),
                "https://example.invalid",
                400
            )
            .is_none()
        );
        assert!(
            verify(
                &(payload + "a"),
                &sig,
                key.public_key().as_ref(),
                "https://example.invalid",
                101
            )
            .is_none()
        );
    }
}
