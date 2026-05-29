//! Session signing-key secret.

use rand::{rngs::OsRng, RngCore};
use sqlx::SqlitePool;

use super::AuthResult;

const SESSION_KEY_NAME: &str = "session_signing_key";
const SESSION_KEY_LEN: usize = 64; // 512 bits — tower-sessions key size

/// Returns the session signing key. Creates and persists a fresh random
/// key on first call if none exists. Operators who want to manage it
/// externally can pre-seed this row (or set `OMNIBUS_SESSION_KEY` — server
/// layer reads the env var and calls `put_session_key` at boot).
pub async fn load_or_create_session_key(pool: &SqlitePool) -> AuthResult<Vec<u8>> {
    if let Some(bytes) = get_session_key(pool).await? {
        return Ok(bytes);
    }
    let mut key = vec![0u8; SESSION_KEY_LEN];
    OsRng.fill_bytes(&mut key);
    put_session_key(pool, &key).await?;
    Ok(key)
}

pub async fn get_session_key(pool: &SqlitePool) -> AuthResult<Option<Vec<u8>>> {
    let v: Option<Vec<u8>> = sqlx::query_scalar("SELECT value FROM secrets WHERE name = ?")
        .bind(SESSION_KEY_NAME)
        .fetch_optional(pool)
        .await?;
    Ok(v)
}

pub async fn put_session_key(pool: &SqlitePool, key: &[u8]) -> AuthResult<()> {
    sqlx::query(
        "INSERT INTO secrets (name, value) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
    )
    .bind(SESSION_KEY_NAME)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::pool;

    #[tokio::test]
    async fn session_key_is_created_and_stable() {
        let p = pool().await;
        let k1 = load_or_create_session_key(&p).await.unwrap();
        let k2 = load_or_create_session_key(&p).await.unwrap();
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), SESSION_KEY_LEN);
    }
}
