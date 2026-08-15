//! Token generation + at-rest hashing.

use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

use super::{AuthError, AuthResult, SessionKind};

/// 32 bytes from the OS CSPRNG via `getrandom`, base64url-encoded (no
/// padding). ~43 chars, 256-bit entropy. Returned to the client exactly once.
pub fn generate_token() -> AuthResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| AuthError::Crypto(format!("CSPRNG byte generation failed: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of the raw token. What we store and look up by.
pub fn hash_token(raw: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().to_vec()
}

/// Plain cookie name for cookie-mode sessions on plain-HTTP dev origins
/// (loopback / LAN IPs without TLS). Centralized here so HTTP-side
/// callers (server's `AuthUser` extractor + the rpc.rs body-side
/// equivalent) don't drift apart.
pub const SESSION_COOKIE_NAME: &str = "omnibus_session";

/// `__Host-` prefixed cookie name used on HTTPS deployments (RFC 6265bis).
/// The prefix is a browser-enforced contract: the cookie MUST be
/// `Secure`, MUST NOT have a `Domain` attribute, and MUST have `Path=/`
/// — together this prevents subdomain-based cookie injection from a
/// sibling `evil.example.com` against `omnibus.example.com`. The
/// write-side guarantees those attributes in `server::auth::handlers`.
pub const SESSION_COOKIE_NAME_HOST_PREFIXED: &str = "__Host-omnibus_session";

/// Returns true if `name` is either of the recognized session cookie
/// names. Used so the same parser accepts both the plain dev name and
/// the `__Host-` prefixed production name without the read side having
/// to know which the server is currently writing.
pub fn is_session_cookie_name(name: &str) -> bool {
    name == SESSION_COOKIE_NAME || name == SESSION_COOKIE_NAME_HOST_PREFIXED
}

/// Pull a session token out of HTTP request headers, preferring an
/// `Authorization: Bearer …` value over the session cookie. Returns
/// `None` when neither source has a non-empty token.
///
/// Recognizes both the plain `omnibus_session` cookie and the
/// `__Host-omnibus_session` production form — the server picks one to
/// write based on `OMNIBUS_SECURE_COOKIES`, but the parser stays
/// permissive so a deploy that flips the flag mid-rollout doesn't
/// invalidate in-flight cookies on the wrong path.
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
        // rather than pulling in axum-extra's CookieJar. If both the
        // `__Host-`-prefixed and plain names are present, the
        // `__Host-`-prefixed value wins because that's the stronger
        // attestation; only fall back to the plain name if no prefixed
        // form had a non-empty value.
        let mut fallback: Option<String> = None;
        for pair in cookies.split(';') {
            let pair = pair.trim();
            if let Some((name, value)) = pair.split_once('=') {
                let name = name.trim();
                let token = value.trim();
                if token.is_empty() {
                    continue;
                }
                if name == SESSION_COOKIE_NAME_HOST_PREFIXED {
                    return Some((token.to_string(), SessionKind::Cookie));
                }
                if name == SESSION_COOKIE_NAME && fallback.is_none() {
                    fallback = Some(token.to_string());
                }
            }
        }
        if let Some(token) = fallback {
            return Some((token, SessionKind::Cookie));
        }
    }
    None
}

/// Parse `Authorization: Basic <base64(user:pass)>` credentials (RFC 7617).
/// Returns `None` for any other scheme, undecodable base64, non-UTF-8
/// bytes, or a payload with no `:` separator. The password keeps any `:`
/// it contains — only the first separator splits.
///
/// Pure-string API like [`parse_session_token`], for the same reason: the
/// server's OPDS Basic-auth extractor passes the header value through.
pub fn parse_basic_credentials(authorization: &str) -> Option<(String, String)> {
    let encoded = authorization.strip_prefix("Basic ")?.trim();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (username, password) = text.split_once(':')?;
    if username.is_empty() {
        return None;
    }
    Some((username.to_string(), password.to_string()))
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

    #[test]
    fn parse_token_accepts_plain_session_cookie() {
        let got = parse_session_token(None, Some("omnibus_session=raw-token"));
        assert_eq!(got, Some(("raw-token".to_string(), SessionKind::Cookie)));
    }

    #[test]
    fn parse_token_accepts_host_prefixed_session_cookie() {
        let got = parse_session_token(None, Some("__Host-omnibus_session=raw-token"));
        assert_eq!(got, Some(("raw-token".to_string(), SessionKind::Cookie)));
    }

    #[test]
    fn parse_token_prefers_host_prefixed_when_both_present() {
        // A deployment that just flipped OMNIBUS_SECURE_COOKIES on may have
        // an in-flight client carrying both names; the prefixed form is
        // the stronger attestation and must win.
        let got = parse_session_token(
            None,
            Some("omnibus_session=legacy; __Host-omnibus_session=prefixed"),
        );
        assert_eq!(got, Some(("prefixed".to_string(), SessionKind::Cookie)));
    }

    #[test]
    fn parse_token_bearer_beats_cookie() {
        let got = parse_session_token(
            Some("Bearer bear-token"),
            Some("__Host-omnibus_session=cookie-token"),
        );
        assert_eq!(got, Some(("bear-token".to_string(), SessionKind::Bearer)));
    }

    #[test]
    fn parse_basic_credentials_decodes_user_and_password() {
        use base64::engine::general_purpose::STANDARD;
        let header = format!("Basic {}", STANDARD.encode("reader:s3cret"));
        let got = parse_basic_credentials(&header);
        assert_eq!(got, Some(("reader".to_string(), "s3cret".to_string())));
    }

    #[test]
    fn parse_basic_credentials_keeps_colons_in_password() {
        use base64::engine::general_purpose::STANDARD;
        let header = format!("Basic {}", STANDARD.encode("reader:pa:ss:word"));
        let got = parse_basic_credentials(&header);
        assert_eq!(got, Some(("reader".to_string(), "pa:ss:word".to_string())));
    }

    #[test]
    fn parse_basic_credentials_rejects_other_schemes_and_garbage() {
        use base64::engine::general_purpose::STANDARD;
        assert_eq!(parse_basic_credentials("Bearer abc"), None);
        assert_eq!(parse_basic_credentials("Basic !!!not-base64!!!"), None);
        // No colon separator in the decoded payload.
        assert_eq!(
            parse_basic_credentials(&format!("Basic {}", STANDARD.encode("no-separator"))),
            None
        );
        // Empty username.
        assert_eq!(
            parse_basic_credentials(&format!("Basic {}", STANDARD.encode(":password"))),
            None
        );
    }

    #[test]
    fn is_session_cookie_name_matches_both_forms() {
        assert!(is_session_cookie_name("omnibus_session"));
        assert!(is_session_cookie_name("__Host-omnibus_session"));
        assert!(!is_session_cookie_name("__Secure-omnibus_session"));
        assert!(!is_session_cookie_name("session"));
    }
}
