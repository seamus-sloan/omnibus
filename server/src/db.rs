use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub ebook_library_path: Option<String>,
    pub audiobook_library_path: Option<String>,
}

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    initialize_schema(&pool).await?;
    Ok(pool)
}

pub async fn initialize_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS app_state (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            value INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO app_state (id, value)
        SELECT 1, 0
        WHERE NOT EXISTS (SELECT 1 FROM app_state WHERE id = 1)
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_value(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let value = sqlx::query_scalar::<_, i64>("SELECT value FROM app_state WHERE id = 1")
        .fetch_one(pool)
        .await?;

    Ok(value)
}

pub async fn increment_value(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE app_state SET value = value + 1 WHERE id = 1")
        .execute(&mut *tx)
        .await?;

    let value = sqlx::query_scalar::<_, i64>("SELECT value FROM app_state WHERE id = 1")
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(value)
}

pub async fn get_settings(pool: &SqlitePool) -> Result<Settings, sqlx::Error> {
    let ebook_library_path = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'ebook_library_path'",
    )
    .fetch_optional(pool)
    .await?;

    let audiobook_library_path = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'audiobook_library_path'",
    )
    .fetch_optional(pool)
    .await?;

    Ok(Settings {
        ebook_library_path,
        audiobook_library_path,
    })
}

pub async fn set_settings(pool: &SqlitePool, settings: &Settings) -> Result<(), sqlx::Error> {
    match &settings.ebook_library_path {
        Some(path) => {
            sqlx::query(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('ebook_library_path', ?)",
            )
            .bind(path)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = 'ebook_library_path'")
                .execute(pool)
                .await?;
        }
    }

    match &settings.audiobook_library_path {
        Some(path) => {
            sqlx::query(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('audiobook_library_path', ?)",
            )
            .bind(path)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = 'audiobook_library_path'")
                .execute(pool)
                .await?;
        }
    }

    Ok(())
}

pub async fn seed_settings_from_env(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let ebook_library_path = std::env::var("EBOOK_LIBRARY_PATH").ok();
    let audiobook_library_path = std::env::var("AUDIOBOOK_LIBRARY_PATH").ok();

    if ebook_library_path.is_some() || audiobook_library_path.is_some() {
        set_settings(
            pool,
            &Settings {
                ebook_library_path,
                audiobook_library_path,
            },
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initializes_and_seeds_default_value() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let value = get_value(&pool).await.expect("seeded value should exist");
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn increments_value_persistently() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");

        let value = increment_value(&pool)
            .await
            .expect("increment should succeed");
        assert_eq!(value, 1);

        let value = get_value(&pool).await.expect("value should be persisted");
        assert_eq!(value, 1);
    }

    #[tokio::test]
    async fn get_settings_returns_none_for_empty_db() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let settings = get_settings(&pool).await.expect("should succeed");
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn set_and_get_settings_roundtrips() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let input = Settings {
            ebook_library_path: Some("/books/ebooks".to_string()),
            audiobook_library_path: Some("/books/audio".to_string()),
        };
        set_settings(&pool, &input)
            .await
            .expect("set should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result, input);
    }

    #[tokio::test]
    async fn set_settings_updates_existing_values() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("first set should succeed");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".to_string()),
                audiobook_library_path: Some("/audio".to_string()),
            },
        )
        .await
        .expect("second set should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, Some("/new".to_string()));
        assert_eq!(result.audiobook_library_path, Some("/audio".to_string()));
    }

    #[tokio::test]
    async fn set_settings_none_clears_existing_value() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".to_string()),
                audiobook_library_path: Some("/audio".to_string()),
            },
        )
        .await
        .expect("set should succeed");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .expect("clear should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn seed_settings_from_env_writes_env_vars_to_db() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        // Use unique env var names per test to avoid cross-test pollution
        std::env::set_var("EBOOK_LIBRARY_PATH", "/env/books");
        std::env::set_var("AUDIOBOOK_LIBRARY_PATH", "/env/audio");
        seed_settings_from_env(&pool)
            .await
            .expect("seed should succeed");
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, Some("/env/books".to_string()));
        assert_eq!(
            result.audiobook_library_path,
            Some("/env/audio".to_string())
        );
    }

    #[tokio::test]
    async fn seed_settings_from_env_is_noop_when_vars_unset() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        // Ensure the vars aren't set (they shouldn't be in a clean test env)
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        seed_settings_from_env(&pool)
            .await
            .expect("seed should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }
}
