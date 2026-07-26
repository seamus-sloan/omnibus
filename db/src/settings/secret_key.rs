//! Shared admin-secret KV field: the get/set/effective/status/seed-from-env
//! shape behind both the Hardcover and Google Books API keys. Each key is one
//! [`SecretKeySpec`] (KV key, env var, max byte length, label); the public
//! wrappers in the parent module supply the constants and adapt the neutral
//! [`SecretKeyStatus`] into their wire types.

use sqlx::SqlitePool;

use super::{mask_key, non_empty_env, SettingsError};

/// The constants that distinguish one admin-secret field from another.
pub(super) struct SecretKeySpec {
    /// `settings` KV key the value is stored under.
    pub kv_key: &'static str,
    /// Env var consulted as the fallback when nothing is saved.
    pub env_var: &'static str,
    /// Max accepted byte length; a longer value is rejected before the write.
    pub max_len: usize,
    /// Human noun for the length-validation message ("hardcover api key").
    pub label: &'static str,
}

/// Neutral masked status: settings wins, then env, then unset. Adapted into the
/// per-key wire type ([`super::HardcoverKeyStatus`] /
/// [`super::GoogleBooksKeyStatus`]) by the caller.
pub(super) struct SecretKeyStatus {
    pub configured: bool,
    pub masked: Option<String>,
    pub source: String,
}

impl SecretKeySpec {
    /// Read the saved secret, or `None` when unset/blank. This is the raw
    /// secret — callers serving it to a client MUST mask it.
    pub(super) async fn get(&self, pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
        let v = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(self.kv_key)
            .fetch_optional(pool)
            .await?;
        Ok(v.filter(|s| !s.trim().is_empty()))
    }

    /// Persist (or clear, when `None`/blank) the secret. Rejects a value longer
    /// than `max_len` with [`SettingsError::Validation`] before the write so no
    /// admin write path can spill an unbounded blob into the `settings` table.
    pub(super) async fn set(
        &self,
        pool: &SqlitePool,
        key: Option<&str>,
    ) -> Result<(), SettingsError> {
        match key.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) if v.len() > self.max_len => Err(SettingsError::Validation(format!(
                "{} exceeds {} bytes",
                self.label, self.max_len
            ))),
            Some(v) => {
                sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                    .bind(self.kv_key)
                    .bind(v)
                    .execute(pool)
                    .await?;
                Ok(())
            }
            None => {
                sqlx::query("DELETE FROM settings WHERE key = ?")
                    .bind(self.kv_key)
                    .execute(pool)
                    .await?;
                Ok(())
            }
        }
    }

    /// The effective secret: the saved settings value wins, then the env var,
    /// else `None` (feature disabled / keyless).
    pub(super) async fn effective(
        &self,
        pool: &SqlitePool,
    ) -> Result<Option<String>, SettingsError> {
        if let Some(saved) = self.get(pool).await? {
            return Ok(Some(saved));
        }
        Ok(non_empty_env(self.env_var))
    }

    /// Masked status: settings wins (`source = "settings"`), then env
    /// (`source = "env"`), else unset (`source = "none"`). Never returns the raw
    /// key — only a short masked preview.
    pub(super) async fn status(&self, pool: &SqlitePool) -> Result<SecretKeyStatus, SettingsError> {
        if let Some(k) = self.get(pool).await? {
            return Ok(SecretKeyStatus {
                configured: true,
                masked: Some(mask_key(&k)),
                source: "settings".to_string(),
            });
        }
        if let Some(env_key) = non_empty_env(self.env_var) {
            return Ok(SecretKeyStatus {
                configured: true,
                masked: Some(mask_key(&env_key)),
                source: "env".to_string(),
            });
        }
        Ok(SecretKeyStatus {
            configured: false,
            masked: None,
            source: "none".to_string(),
        })
    }

    /// Boot hook: seed the secret from its env var **only** when no value is
    /// already saved, so the env var works out of the box without clobbering a
    /// key set through Settings on every restart (settings wins).
    pub(super) async fn seed_from_env(&self, pool: &SqlitePool) -> Result<(), SettingsError> {
        if self.get(pool).await?.is_some() {
            return Ok(());
        }
        if let Some(env_key) = non_empty_env(self.env_var) {
            self.set(pool, Some(&env_key)).await?;
        }
        Ok(())
    }
}
