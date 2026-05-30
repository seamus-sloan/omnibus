//! Password hashing + policy.

use argon2::password_hash::{
    rand_core::OsRng as PhcOsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};

use super::{AuthError, AuthResult};

/// OWASP 2024 floor for Argon2id. Hardcoded, not configurable — if we ever
/// need to tune these, rotation is free (on verify we rehash if the stored
/// PHC string's parameters are below current policy).
const ARGON2_MEMORY_KIB: u32 = 19_456; // 19 MiB
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

const MIN_PASSWORD_LEN: usize = 10;
const MAX_PASSWORD_LEN: usize = 128;

/// Username length ceiling, measured in Unicode scalar values. Sized to
/// comfortably cover real names, role-style handles, and email-derived
/// usernames while keeping the value short enough to render in any UI
/// column and to bound storage/log overhead per row.
const MAX_USERNAME_LEN: usize = 64;

/// Tiny embedded reject-list. Deliberately small (top ~50) — this is a
/// "don't be stupid" check, not a HIBP replacement. Self-hosted deployments
/// are offline-tolerant, so a runtime breach check is out of scope.
const COMMON_PASSWORDS: &[&str] = &[
    "password",
    "password1",
    "password12",
    "password123",
    "password1234",
    "12345678",
    "123456789",
    "1234567890",
    "qwerty123",
    "qwertyuiop",
    "letmein123",
    "welcome123",
    "admin1234",
    "administrator",
    "iloveyou1",
    "dragon1234",
    "sunshine1",
    "princess1",
    "football1",
    "baseball1",
    "superman1",
    "batman1234",
    "trustno1234",
    "shadow1234",
    "master1234",
    "qazwsxedc",
    "zxcvbnm123",
    "asdfghjkl1",
    "11111111",
    "00000000",
    "12341234",
    "abcd1234",
    "passw0rd",
    "p@ssw0rd1",
    "qwerty1234",
    "monkey1234",
    "hello1234",
    "loveyou123",
    "liverpool1",
    "arsenal1",
    "chelsea123",
    "tottenham1",
    "manchester1",
    "brooklyn1",
    "jennifer1",
    "michelle1",
    "computer1",
    "internet1",
];

pub(crate) fn argon2_hasher() -> Argon2<'static> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        None,
    )
    .expect("argon2 params are compile-time valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn validate_password(password: &str) -> AuthResult<()> {
    // Count Unicode scalar values, not bytes, so a few emoji can't satisfy
    // a byte-length floor. Argon2 itself is byte-oriented and imposes no
    // separate limit; MAX_PASSWORD_LEN guards against unbounded CPU work.
    let char_count = password.chars().count();
    if char_count < MIN_PASSWORD_LEN {
        return Err(AuthError::PasswordTooShort {
            min: MIN_PASSWORD_LEN,
        });
    }
    if char_count > MAX_PASSWORD_LEN {
        return Err(AuthError::PasswordTooLong {
            max: MAX_PASSWORD_LEN,
        });
    }
    let lower = password.to_lowercase();
    if COMMON_PASSWORDS.iter().any(|c| *c == lower) {
        return Err(AuthError::PasswordCommon);
    }
    Ok(())
}

/// Validate a username before insert. Rules:
///
/// * Non-empty after rejecting leading/trailing whitespace.
/// * Maximum `MAX_USERNAME_LEN` Unicode scalar values (not bytes), so a
///   handful of multi-byte characters can't cheaply blow past the cap.
/// * No leading or trailing whitespace — those are almost always a paste
///   accident and the lack of normalization would let "alice" and
///   " alice" coexist as distinct rows under `COLLATE NOCASE`.
/// * No ASCII control characters (U+0000..=U+001F, U+007F). Null bytes
///   break C-string-shaped consumers; other controls corrupt log output
///   and terminal/UI rendering.
///
/// Deliberately *not* handled here: Unicode homoglyph confusables
/// (e.g. Cyrillic `а` vs Latin `a`). That's a normalization/display
/// concern that belongs alongside the admin UI surfaces that render
/// user-supplied usernames; case-collision dedup via the existing
/// `users.username COLLATE NOCASE` index is the only collision guarantee
/// this layer makes.
pub fn validate_username(username: &str) -> AuthResult<()> {
    if username.is_empty() {
        return Err(AuthError::UsernameEmpty);
    }
    if username.trim() != username {
        return Err(AuthError::UsernameWhitespace);
    }
    // Re-check after trim in case the input was entirely whitespace —
    // `trim() != self` would have already caught that, but the empty check
    // here makes the intent explicit if the rules above ever reorder.
    if username.trim().is_empty() {
        return Err(AuthError::UsernameEmpty);
    }
    if username.chars().count() > MAX_USERNAME_LEN {
        return Err(AuthError::UsernameTooLong {
            max: MAX_USERNAME_LEN,
        });
    }
    if username
        .chars()
        .any(|c| (c as u32) <= 0x1F || c as u32 == 0x7F)
    {
        return Err(AuthError::UsernameInvalidChar);
    }
    Ok(())
}

pub fn hash_password(password: &str) -> AuthResult<String> {
    let salt = SaltString::generate(&mut PhcOsRng);
    let phc = argon2_hasher()
        .hash_password(password.as_bytes(), &salt)?
        .to_string();
    Ok(phc)
}

/// Verify a password against a stored PHC hash. Constant-time via argon2's
/// internal equality check. Returns Ok(true) only on match.
pub fn verify_password(password: &str, phc: &str) -> AuthResult<bool> {
    let parsed = PasswordHash::new(phc)?;
    match argon2_hasher().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrips() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(phc.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &phc).unwrap());
        assert!(!verify_password("wrong password entirely", &phc).unwrap());
    }

    #[test]
    fn password_policy_rejects_short() {
        assert!(matches!(
            validate_password("short"),
            Err(AuthError::PasswordTooShort { .. })
        ));
    }

    #[test]
    fn password_policy_rejects_common() {
        assert!(matches!(
            validate_password("password123"),
            Err(AuthError::PasswordCommon)
        ));
    }

    #[test]
    fn password_policy_accepts_reasonable() {
        assert!(validate_password("xk7-banana-frog-42").is_ok());
    }

    // ---- username policy ----------------------------------------------------

    #[test]
    fn username_policy_rejects_empty() {
        assert!(matches!(
            validate_username(""),
            Err(AuthError::UsernameEmpty)
        ));
    }

    #[test]
    fn username_policy_accepts_single_char() {
        assert!(validate_username("a").is_ok());
    }

    #[test]
    fn username_policy_accepts_max_length() {
        let name: String = "a".repeat(MAX_USERNAME_LEN);
        assert!(validate_username(&name).is_ok());
    }

    #[test]
    fn username_policy_rejects_over_max_length() {
        let name: String = "a".repeat(MAX_USERNAME_LEN + 1);
        assert!(matches!(
            validate_username(&name),
            Err(AuthError::UsernameTooLong { .. })
        ));
    }

    #[test]
    fn username_policy_rejects_leading_whitespace() {
        assert!(matches!(
            validate_username(" alice"),
            Err(AuthError::UsernameWhitespace)
        ));
    }

    #[test]
    fn username_policy_rejects_trailing_whitespace() {
        assert!(matches!(
            validate_username("alice "),
            Err(AuthError::UsernameWhitespace)
        ));
    }

    #[test]
    fn username_policy_rejects_only_whitespace() {
        // All-whitespace input has trim() != self, so it surfaces as the
        // whitespace error rather than the empty error — either is a
        // reject, but lock the variant to keep callers' error UX stable.
        assert!(matches!(
            validate_username("   "),
            Err(AuthError::UsernameWhitespace)
        ));
    }

    #[test]
    fn username_policy_rejects_embedded_tab() {
        assert!(matches!(
            validate_username("ali\tce"),
            Err(AuthError::UsernameInvalidChar)
        ));
    }

    #[test]
    fn username_policy_rejects_embedded_newline() {
        assert!(matches!(
            validate_username("ali\nce"),
            Err(AuthError::UsernameInvalidChar)
        ));
    }

    #[test]
    fn username_policy_rejects_embedded_null() {
        assert!(matches!(
            validate_username("ali\0ce"),
            Err(AuthError::UsernameInvalidChar)
        ));
    }

    #[test]
    fn username_policy_rejects_low_control_char() {
        assert!(matches!(
            validate_username("ali\x1fce"),
            Err(AuthError::UsernameInvalidChar)
        ));
    }

    #[test]
    fn username_policy_rejects_delete_char() {
        assert!(matches!(
            validate_username("ali\x7fce"),
            Err(AuthError::UsernameInvalidChar)
        ));
    }

    #[test]
    fn username_policy_accepts_reasonable() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("Alice.Smith-42_").is_ok());
        assert!(validate_username("user@example.com").is_ok());
    }
}
