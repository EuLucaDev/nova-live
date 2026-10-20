//! Server-signed grants for local editing only. Relay access always needs online validation.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{digest, signature};
use serde::{Deserialize, Serialize};

use crate::identity::Profile;

pub const MAX_SECONDS: u64 = 7 * 24 * 60 * 60;
#[derive(Clone, Serialize, Deserialize)]
pub struct Lease {
    pub payload: String,
    pub signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claims {
    version: u32,
    origin: String,
    issued_at: u64,
    expires_at: u64,
    token_hash: String,
    profile: Profile,
}

pub fn token_hash(token: &str) -> String {
    digest::digest(&digest::SHA256, token.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Chave pública Ed25519 do WorkFlow.ind que assina a autorização sem internet e os bilhetes de
/// documento. É pública por natureza; a privada fica só no ambiente da API.
pub const NOVA_FOODS_SIGNING_PUBLIC_KEY_HEX: &str =
    "b91035f67f342ecd5a78fd109b60ac7db3bf4e428a68f18a8c878c24e0e6930d";

/// Public key comes from the build, never from the mutable session cache. A build may override
/// it with `NOVA_OFFLINE_PUBLIC_KEY_HEX` (tests, another deployment).
pub fn configured_key() -> Option<Vec<u8>> {
    let hex =
        option_env!("NOVA_OFFLINE_PUBLIC_KEY_HEX").unwrap_or(NOVA_FOODS_SIGNING_PUBLIC_KEY_HEX);
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            let s = std::str::from_utf8(c).ok()?;
            u8::from_str_radix(s, 16).ok()
        })
        .collect()
}

impl Lease {
    pub fn verify(
        &self,
        public_key: &[u8],
        token: &str,
        origin: &str,
        now: u64,
    ) -> Option<Profile> {
        if self.payload.len() > 128 * 1024 || self.signature.len() > 128 {
            return None;
        }
        let sig = URL_SAFE_NO_PAD.decode(&self.signature).ok()?;
        signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
            .verify(self.payload.as_bytes(), &sig)
            .ok()?;
        let claims: Claims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&self.payload).ok()?).ok()?;
        if claims.version != 1
            || claims.origin != origin
            || claims.token_hash != token_hash(token)
            || claims.issued_at > now
            || claims.expires_at <= now
            || claims.expires_at.checked_sub(claims.issued_at)? > MAX_SECONDS
            || claims.profile.user.name.trim().is_empty()
            || claims.profile.apps.len() > 13
            || claims.profile.departments.len() > 250
        {
            return None;
        }
        Some(claims.profile)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    fn profile() -> Profile {
        serde_json::from_value(serde_json::json!({
            "user":{"id":"u","name":"Test","avatarUrl":null},
            "organization":{"id":"o","name":"Test"},"departments":[],
            "apps":["novaphoto"],"policyEnabled":true,"canManage":false
        }))
        .unwrap()
    }
    #[test]
    fn grant_is_bound_to_device_origin_permissions_and_expiration() {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let claims = Claims {
            version: 1,
            origin: "https://example.invalid".into(),
            issued_at: 100,
            expires_at: 100 + MAX_SECONDS,
            token_hash: token_hash("device"),
            profile: profile(),
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let mut lease = Lease {
            signature: URL_SAFE_NO_PAD.encode(pair.sign(payload.as_bytes()).as_ref()),
            payload,
        };
        let key = pair.public_key().as_ref();
        assert!(
            lease
                .verify(key, "device", "https://example.invalid", 101)
                .unwrap()
                .allows("novaphoto")
        );
        assert!(
            lease
                .verify(key, "other", "https://example.invalid", 101)
                .is_none()
        );
        assert!(
            lease
                .verify(key, "device", "https://other.invalid", 101)
                .is_none()
        );
        assert!(
            lease
                .verify(key, "device", "https://example.invalid", 99)
                .is_none()
        );
        assert!(
            lease
                .verify(key, "device", "https://example.invalid", 100 + MAX_SECONDS)
                .is_none()
        );
        lease.payload.push('a');
        assert!(
            lease
                .verify(key, "device", "https://example.invalid", 101)
                .is_none()
        );
    }
}
