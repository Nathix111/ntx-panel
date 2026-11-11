use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use tonic::{Request, Status};

pub const AUTH_HEADER: &str = "x-ntx-auth";

pub fn hash_psk(psk: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(psk.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Clone, Debug)]
pub struct AuthManager {
    psk_hash: String,
    sessions: Arc<RwLock<HashSet<String>>>,
}

impl AuthManager {
    pub fn new(psk: &str) -> Self {
        Self {
            psk_hash: hash_psk(psk),
            sessions: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    #[allow(dead_code)]
    pub fn psk_hash(&self) -> &str {
        &self.psk_hash
    }

    pub fn verify_psk_hash(&self, provided_hash: &str) -> bool {
        self.psk_hash.eq_ignore_ascii_case(provided_hash.trim())
    }

    pub fn create_session(&self) -> String {
        let session_token = uuid::Uuid::new_v4().to_string();
        if let Ok(mut lock) = self.sessions.write() {
            lock.insert(session_token.clone());
        }
        session_token
    }

    pub fn is_valid_token(&self, token: &str) -> bool {
        let trimmed = token.trim();
        if self.verify_psk_hash(trimmed) {
            return true;
        }

        if let Ok(lock) = self.sessions.read() {
            if lock.contains(trimmed) {
                return true;
            }
        }
        false
    }

    #[allow(clippy::result_large_err)]
    pub fn verify_request<T>(&self, req: &Request<T>) -> Result<(), Status> {
        let token_val = req
            .metadata()
            .get(AUTH_HEADER)
            .ok_or_else(|| Status::unauthenticated("Missing authentication header"))?;

        let token_str = token_val
            .to_str()
            .map_err(|_| Status::unauthenticated("Invalid auth token encoding"))?;

        if self.is_valid_token(token_str) {
            Ok(())
        } else {
            Err(Status::unauthenticated("Invalid authentication token"))
        }
    }

    /// Interceptor check: if header is present, must be valid.
    #[allow(clippy::result_large_err)]
    pub fn intercept(&self, req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token_val) = req.metadata().get(AUTH_HEADER) {
            let token_str = token_val
                .to_str()
                .map_err(|_| Status::unauthenticated("Invalid auth token encoding"))?;
            if !self.is_valid_token(token_str) {
                return Err(Status::unauthenticated("Invalid authentication token"));
            }
        }
        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use tonic::metadata::MetadataValue;

    #[test]
    fn test_psk_hashing() {
        let psk = "secret123";
        let hash = hash_psk(psk);
        // Pre-computed sha256 of "secret123":
        // 5ebe2294ecd0e0f08eab7690d2a6ee69faf6ff780e86b4b64aa57f12e080cb9e (wait, let's verify)
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_psk(psk));
        assert_ne!(hash, hash_psk("other"));
    }

    #[test]
    fn test_auth_manager() {
        let auth = AuthManager::new("my-test-psk");
        let hash = auth.psk_hash().to_string();

        // 1. Direct hash verification
        assert!(auth.verify_psk_hash(&hash));
        assert!(auth.verify_psk_hash(&hash.to_uppercase()));
        assert!(!auth.verify_psk_hash("wrong-hash"));

        // 2. Direct token check
        assert!(auth.is_valid_token(&hash));

        // 3. Session tokens
        let session = auth.create_session();
        assert!(auth.is_valid_token(&session));
        assert!(!auth.is_valid_token("random-token"));

        // 4. Request metadata check
        let mut req = Request::new(());
        assert!(auth.verify_request(&req).is_err());

        req.metadata_mut()
            .insert(AUTH_HEADER, MetadataValue::from_str(&session).unwrap());
        assert!(auth.verify_request(&req).is_ok());

        // 5. Interceptor
        let intercept_req = Request::new(());
        // Unauthenticated passes through interceptor so Authenticate / Ping can be accessed
        assert!(auth.intercept(intercept_req).is_ok());

        let mut bad_req = Request::new(());
        bad_req
            .metadata_mut()
            .insert(AUTH_HEADER, MetadataValue::from_str("bad-token").unwrap());
        assert!(auth.intercept(bad_req).is_err());
    }
}
