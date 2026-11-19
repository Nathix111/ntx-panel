use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// Derive a 32-byte key from a Pre-Shared Key (PSK) using SHA-256.
pub fn derive_psk_key(psk: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(psk.as_bytes());
    hasher.finalize().into()
}

/// Compute the challenge-response HMAC-SHA256 hex digest matching `ntx-server`.
pub fn compute_challenge_hmac(psk: &str, nonce: &[u8], viewer_id: &str) -> String {
    let key = derive_psk_key(psk);
    let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.update(b":");
    mac.update(viewer_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Generate a new unique viewer ID (UUID v4).
pub fn generate_viewer_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_challenge_hmac() {
        let psk = "secret123";
        let nonce = b"nonce_bytes";
        let viewer = "viewer-1";
        let mac1 = compute_challenge_hmac(psk, nonce, viewer);
        let mac2 = compute_challenge_hmac(psk, nonce, viewer);
        assert_eq!(mac1.len(), 64);
        assert_eq!(mac1, mac2);
        assert_ne!(mac1, compute_challenge_hmac("other", nonce, viewer));
        assert_ne!(mac1, compute_challenge_hmac(psk, b"other", viewer));
        assert_ne!(mac1, compute_challenge_hmac(psk, nonce, "viewer-2"));
    }

    #[test]
    fn test_generate_viewer_id() {
        let id1 = generate_viewer_id();
        let id2 = generate_viewer_id();
        assert_ne!(id1, id2);
        assert!(Uuid::parse_str(&id1).is_ok());
    }
}
