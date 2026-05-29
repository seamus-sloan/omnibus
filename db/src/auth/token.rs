//! Token generation + at-rest hashing.

use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

use super::{AuthError, AuthResult, SessionKind};

/// 32 bytes from the OS CSPRNG via `getrandom`, base64url-encoded (no
/// padding). ~43 chars, 256-bit entropy. Returned to the client exactly once.
pub fn generate_token() -> AuthResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| AuthError::TokenGeneration(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of the raw token. What we store and look up by.
pub fn hash_token(raw: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().to_vec()
}

/// Cookie name for cookie-mode sessions. Centralized here so HTTP-side
/// callers (server's `AuthUser` extractor + the rpc.rs body-side
/// equivalent) don't drift apart.
pub const SESSION_COOKIE_NAME: &str = "omnibus_session";

/// Pull a session token out of HTTP request headers, preferring an
/// `Authorization: Bearer …` value over the `omnibus_session` cookie.
/// Returns `None` when neither source has a non-empty token.
///
/// Pure-string API by design — keeps `omnibus-db` free of an axum/http
/// type dependency. Callers pass the relevant header values through.
pub fn parse_session_token(
    authorization: Option<&str>,
    cookie_header: Option<&str>,
) -> Option<(String, SessionKind)> {
    if let Some(value) = authorization {
        if let Some(rest) = value.strip_prefix("Bearer ") {
            let token = rest.trim();
            if !token.is_empty() {
                return Some((token.to_string(), SessionKind::Bearer));
            }
        }
    }
    if let Some(cookies) = cookie_header {
        // Cookie header is `name1=value1; name2=value2`. Walk it manually
        // rather than pulling in axum-extra's CookieJar.
        for pair in cookies.split(';') {
            let pair = pair.trim();
            if let Some((name, value)) = pair.split_once('=') {
                if name.trim() == SESSION_COOKIE_NAME {
                    let token = value.trim();
                    if !token.is_empty() {
                        return Some((token.to_string(), SessionKind::Cookie));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_unique_and_base64url() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_ne!(a, b);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(a.len() >= 40);
    }

    #[test]
    fn token_hash_is_deterministic_and_32_bytes() {
        let t = "abc123";
        assert_eq!(hash_token(t), hash_token(t));
        assert_eq!(hash_token(t).len(), 32);
    }
}
