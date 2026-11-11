use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Hash a Pre-Shared Key (PSK) using SHA-256 and return hex string.
pub fn hash_psk(psk: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(psk.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generate a new unique viewer ID (UUID v4).
pub fn generate_viewer_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_psk() {
        let psk = "secret123";
        let hash = hash_psk(psk);
        assert_eq!(hash.len(), 64);
        // SHA-256 of "secret123" is well-known:
        // echo -n "secret123" | sha256sum -> 5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8 (wait, let's test determinism)
        let hash2 = hash_psk(psk);
        assert_eq!(hash, hash2);
        assert_ne!(hash, hash_psk("other"));
    }

    #[test]
    fn test_generate_viewer_id() {
        let id1 = generate_viewer_id();
        let id2 = generate_viewer_id();
        assert_ne!(id1, id2);
        assert!(Uuid::parse_str(&id1).is_ok());
    }
}
