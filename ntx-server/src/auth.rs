use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tonic::{Request, Status};

pub const AUTH_HEADER: &str = "x-ntx-auth";

const CHALLENGE_TTL: Duration = Duration::from_secs(30);
const MAX_CHALLENGES: usize = 256;
const MAX_SESSIONS: usize = 1024;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_BLOCK_DURATION: Duration = Duration::from_secs(60);
const RATE_LIMIT_MAX_FAILURES: u32 = 5;
const MAX_RATE_LIMIT_IPS: usize = 4096;

type HmacSha256 = Hmac<Sha256>;

pub fn derive_psk_key(psk: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(psk.as_bytes());
    hasher.finalize().into()
}

pub fn compute_challenge_hmac(psk: &str, nonce: &[u8], viewer_id: &str) -> String {
    let key = derive_psk_key(psk);
    let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.update(b":");
    mac.update(viewer_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Debug, Clone)]
struct ChallengeEntry {
    nonce: Vec<u8>,
    viewer_id: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct SessionEntry {
    #[allow(dead_code)]
    viewer_id: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct RateLimitEntry {
    failures: Vec<Instant>,
    blocked_until: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFailure {
    RateLimited(String),
    Rejected(String),
}

#[derive(Clone, Debug)]
pub struct AuthManager {
    psk_key: [u8; 32],
    session_ttl: Duration,
    challenges: Arc<RwLock<HashMap<String, ChallengeEntry>>>,
    sessions: Arc<RwLock<HashMap<String, SessionEntry>>>,
    rate_limits: Arc<RwLock<HashMap<String, RateLimitEntry>>>,
}

impl AuthManager {
    pub fn new(psk: &str, session_ttl_secs: u64) -> Self {
        Self::with_session_duration(psk, Duration::from_secs(session_ttl_secs.max(60)))
    }

    fn with_session_duration(psk: &str, session_ttl: Duration) -> Self {
        Self {
            psk_key: derive_psk_key(psk),
            session_ttl,
            challenges: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn check_rate_limit(&self, peer_key: &str) -> Result<(), Status> {
        let now = Instant::now();
        let mut lock = self
            .rate_limits
            .write()
            .map_err(|_| Status::internal("Auth lock poisoned"))?;

        lock.retain(|_, entry| {
            let active_block = entry.blocked_until.map(|b| b > now).unwrap_or(false);
            let recent_fail = entry
                .failures
                .iter()
                .any(|&t| now.saturating_duration_since(t) < RATE_LIMIT_WINDOW);
            active_block || recent_fail
        });

        if let Some(entry) = lock.get_mut(peer_key) {
            if let Some(blocked_until) = entry.blocked_until {
                if now < blocked_until {
                    let remaining = blocked_until.saturating_duration_since(now).as_secs().max(1);
                    return Err(Status::resource_exhausted(format!(
                        "Too many failed authentication attempts. Try again in {}s",
                        remaining
                    )));
                } else {
                    entry.blocked_until = None;
                    entry.failures.clear();
                }
            }
        }

        Ok(())
    }

    pub fn record_auth_failure(&self, peer_key: &str) {
        let now = Instant::now();
        if let Ok(mut lock) = self.rate_limits.write() {
            lock.retain(|_, entry| {
                let active_block = entry.blocked_until.map(|b| b > now).unwrap_or(false);
                let recent_fail = entry
                    .failures
                    .iter()
                    .any(|&t| now.saturating_duration_since(t) < RATE_LIMIT_WINDOW);
                active_block || recent_fail
            });

            if !lock.contains_key(peer_key) && lock.len() >= MAX_RATE_LIMIT_IPS {
                if let Some(oldest_key) = lock
                    .iter()
                    .min_by_key(|(_, e)| e.failures.last().copied().unwrap_or(now))
                    .map(|(k, _)| k.clone())
                {
                    lock.remove(&oldest_key);
                }
            }

            let entry = lock
                .entry(peer_key.to_string())
                .or_insert_with(|| RateLimitEntry {
                    failures: Vec::new(),
                    blocked_until: None,
                });

            entry
                .failures
                .retain(|&t| now.saturating_duration_since(t) < RATE_LIMIT_WINDOW);
            entry.failures.push(now);

            if entry.failures.len() as u32 >= RATE_LIMIT_MAX_FAILURES {
                entry.blocked_until = Some(now + RATE_LIMIT_BLOCK_DURATION);
            }
        }
    }

    pub fn record_auth_success(&self, peer_key: &str) {
        if let Ok(mut lock) = self.rate_limits.write() {
            lock.remove(peer_key);
        }
    }

    pub fn create_challenge(
        &self,
        peer_key: &str,
        viewer_id: &str,
    ) -> Result<(String, Vec<u8>, u64), Status> {
        self.check_rate_limit(peer_key)?;

        let clean_viewer = viewer_id.trim();
        if clean_viewer.is_empty() {
            return Err(Status::invalid_argument("viewer_id must not be empty"));
        }

        let challenge_id = uuid::Uuid::new_v4().to_string();
        let mut nonce = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let now = Instant::now();
        let expires_at = now + CHALLENGE_TTL;

        if let Ok(mut lock) = self.challenges.write() {
            lock.retain(|_, c| c.expires_at > now);
            while lock.len() >= MAX_CHALLENGES {
                if let Some(oldest_id) = lock
                    .iter()
                    .min_by_key(|(_, c)| c.expires_at)
                    .map(|(id, _)| id.clone())
                {
                    lock.remove(&oldest_id);
                } else {
                    break;
                }
            }
            lock.insert(
                challenge_id.clone(),
                ChallengeEntry {
                    nonce: nonce.clone(),
                    viewer_id: clean_viewer.to_string(),
                    expires_at,
                },
            );
        }

        Ok((challenge_id, nonce, CHALLENGE_TTL.as_secs()))
    }

    pub fn authenticate_challenge(
        &self,
        peer_key: &str,
        challenge_id: &str,
        viewer_id: &str,
        response_hmac_hex: &str,
    ) -> Result<(String, u64), AuthFailure> {
        if let Err(status) = self.check_rate_limit(peer_key) {
            return Err(AuthFailure::RateLimited(status.message().to_string()));
        }

        let clean_challenge_id = challenge_id.trim();
        let clean_viewer_id = viewer_id.trim();
        let clean_hmac = response_hmac_hex.trim();

        if clean_challenge_id.is_empty() || clean_viewer_id.is_empty() || clean_hmac.is_empty() {
            self.record_auth_failure(peer_key);
            return Err(AuthFailure::Rejected(
                "Missing authentication challenge parameters".to_string(),
            ));
        }

        let now = Instant::now();
        let entry = {
            let mut lock = self
                .challenges
                .write()
                .map_err(|_| AuthFailure::Rejected("Internal auth lock error".to_string()))?;
            lock.retain(|_, c| c.expires_at > now);
            lock.remove(clean_challenge_id)
        };

        let Some(challenge) = entry else {
            self.record_auth_failure(peer_key);
            return Err(AuthFailure::Rejected(
                "Invalid or expired authentication challenge".to_string(),
            ));
        };

        if now >= challenge.expires_at || challenge.viewer_id != clean_viewer_id {
            self.record_auth_failure(peer_key);
            return Err(AuthFailure::Rejected(
                "Invalid or expired authentication challenge".to_string(),
            ));
        }

        let Ok(provided_mac) = hex::decode(clean_hmac) else {
            self.record_auth_failure(peer_key);
            return Err(AuthFailure::Rejected(
                "Invalid authentication response format".to_string(),
            ));
        };

        let mut mac = HmacSha256::new_from_slice(&self.psk_key).expect("HMAC accepts any key size");
        mac.update(&challenge.nonce);
        mac.update(b":");
        mac.update(clean_viewer_id.as_bytes());
        let expected_mac = mac.finalize().into_bytes();

        if provided_mac.len() != expected_mac.len()
            || !bool::from(provided_mac.as_slice().ct_eq(expected_mac.as_slice()))
        {
            self.record_auth_failure(peer_key);
            return Err(AuthFailure::Rejected("Invalid PSK credentials".to_string()));
        }

        self.record_auth_success(peer_key);
        let token = self.insert_session(clean_viewer_id);
        Ok((token, self.session_ttl.as_secs()))
    }

    fn insert_session(&self, viewer_id: &str) -> String {
        let mut token_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut token_bytes);
        let session_token = hex::encode(token_bytes);
        let now = Instant::now();
        let expires_at = now + self.session_ttl;

        if let Ok(mut lock) = self.sessions.write() {
            lock.retain(|_, s| s.expires_at > now);
            while lock.len() >= MAX_SESSIONS {
                if let Some(oldest_tok) = lock
                    .iter()
                    .min_by_key(|(_, s)| s.expires_at)
                    .map(|(tok, _)| tok.clone())
                {
                    lock.remove(&oldest_tok);
                } else {
                    break;
                }
            }
            lock.insert(
                session_token.clone(),
                SessionEntry {
                    viewer_id: viewer_id.to_string(),
                    expires_at,
                },
            );
        }

        session_token
    }

    pub fn is_valid_token(&self, token: &str) -> bool {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return false;
        }

        let now = Instant::now();
        if let Ok(lock) = self.sessions.read() {
            if let Some(entry) = lock.get(trimmed) {
                if now < entry.expires_at {
                    return true;
                }
            } else {
                return false;
            }
        }

        // Expired token cleanup
        if let Ok(mut lock) = self.sessions.write() {
            lock.retain(|_, s| s.expires_at > now);
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
            Err(Status::unauthenticated(
                "Invalid or expired authentication token",
            ))
        }
    }

    /// Interceptor check: if header is present, must be a valid active session token.
    #[allow(clippy::result_large_err)]
    pub fn intercept(&self, req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token_val) = req.metadata().get(AUTH_HEADER) {
            let token_str = token_val
                .to_str()
                .map_err(|_| Status::unauthenticated("Invalid auth token encoding"))?;
            if !self.is_valid_token(token_str) {
                return Err(Status::unauthenticated(
                    "Invalid or expired authentication token",
                ));
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
    fn test_challenge_hmac_deterministic_and_sensitive() {
        let psk = "secret123";
        let nonce = b"random_nonce_value";
        let viewer = "viewer-1";
        let mac1 = compute_challenge_hmac(psk, nonce, viewer);
        let mac2 = compute_challenge_hmac(psk, nonce, viewer);
        assert_eq!(mac1.len(), 64);
        assert_eq!(mac1, mac2);
        assert_ne!(mac1, compute_challenge_hmac("other", nonce, viewer));
        assert_ne!(mac1, compute_challenge_hmac(psk, b"other_nonce", viewer));
        assert_ne!(mac1, compute_challenge_hmac(psk, nonce, "viewer-2"));
    }

    #[test]
    fn test_challenge_response_and_replay_protection() {
        let psk = "my-test-psk";
        let auth = AuthManager::new(psk, 3600);
        let peer = "192.168.1.50";
        let viewer = "viewer-abc";

        let (chal_id, nonce, ttl) = auth.create_challenge(peer, viewer).unwrap();
        assert_eq!(ttl, 30);
        assert_eq!(nonce.len(), 32);

        // Wrong HMAC fails
        let wrong_res = auth.authenticate_challenge(peer, &chal_id, viewer, "00".repeat(32).as_str());
        assert!(matches!(wrong_res, Err(AuthFailure::Rejected(_))));

        // Replay of consumed challenge fails even with valid HMAC!
        let valid_hmac = compute_challenge_hmac(psk, &nonce, viewer);
        let replay_res = auth.authenticate_challenge(peer, &chal_id, viewer, &valid_hmac);
        assert!(matches!(replay_res, Err(AuthFailure::Rejected(_))));

        // Fresh challenge with wrong viewer fails
        let (chal_id2, nonce2, _) = auth.create_challenge(peer, viewer).unwrap();
        let hmac_other_viewer = compute_challenge_hmac(psk, &nonce2, "viewer-other");
        let mismatch = auth.authenticate_challenge(peer, &chal_id2, "viewer-other", &hmac_other_viewer);
        assert!(matches!(mismatch, Err(AuthFailure::Rejected(_))));

        // Valid fresh challenge succeeds
        let (chal_id3, nonce3, _) = auth.create_challenge(peer, viewer).unwrap();
        let valid_hmac3 = compute_challenge_hmac(psk, &nonce3, viewer);
        let (session, sess_ttl) = auth
            .authenticate_challenge(peer, &chal_id3, viewer, &valid_hmac3)
            .unwrap();
        assert_eq!(sess_ttl, 3600);
        assert_eq!(session.len(), 64);
        assert!(auth.is_valid_token(&session));

        // Raw psk_hash must NEVER be accepted as an auth token
        let raw_psk_hash = hex::encode(derive_psk_key(psk));
        assert!(!auth.is_valid_token(&raw_psk_hash));

        // Request metadata check
        let mut req = Request::new(());
        assert!(auth.verify_request(&req).is_err());

        req.metadata_mut()
            .insert(AUTH_HEADER, MetadataValue::from_str(&session).unwrap());
        assert!(auth.verify_request(&req).is_ok());

        // Interceptor allows unauthenticated requests (for Ping / GetAuthChallenge / Authenticate)
        assert!(auth.intercept(Request::new(())).is_ok());

        let mut bad_req = Request::new(());
        bad_req
            .metadata_mut()
            .insert(AUTH_HEADER, MetadataValue::from_str(&raw_psk_hash).unwrap());
        assert!(auth.intercept(bad_req).is_err());
    }

    #[test]
    fn test_session_ttl_expiry() {
        let psk = "ttl-psk";
        let auth = AuthManager::with_session_duration(psk, Duration::from_millis(30));
        let (chal_id, nonce, _) = auth.create_challenge("127.0.0.1", "v1").unwrap();
        let hmac = compute_challenge_hmac(psk, &nonce, "v1");
        let (token, _) = auth
            .authenticate_challenge("127.0.0.1", &chal_id, "v1", &hmac)
            .unwrap();

        assert!(auth.is_valid_token(&token));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!auth.is_valid_token(&token));
    }

    #[test]
    fn test_rate_limiting_and_reset() {
        let psk = "rate-limit-psk";
        let auth = AuthManager::new(psk, 3600);
        let attacker = "10.0.0.99";

        for _ in 0..RATE_LIMIT_MAX_FAILURES {
            let (cid, _, _) = auth.create_challenge(attacker, "v1").unwrap();
            let res = auth.authenticate_challenge(attacker, &cid, "v1", &"ab".repeat(32));
            assert!(matches!(res, Err(AuthFailure::Rejected(_))));
        }

        // Next attempt is rate limited
        let chal_blocked = auth.create_challenge(attacker, "v1");
        assert!(chal_blocked.is_err());
        assert_eq!(
            chal_blocked.unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );

        // Different IP is unaffected
        let (cid_ok, nonce_ok, _) = auth.create_challenge("10.0.0.100", "v1").unwrap();
        let hmac_ok = compute_challenge_hmac(psk, &nonce_ok, "v1");
        assert!(auth
            .authenticate_challenge("10.0.0.100", &cid_ok, "v1", &hmac_ok)
            .is_ok());
    }
}
