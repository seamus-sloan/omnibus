//! Placeholder counter from migration `0001`. Read/write the singleton
//! `app_state` row; kept around as the simplest possible smoke check while
//! the real product surfaces (libraries, books, etc.) come online.

use sqlx::SqlitePool;

/// Read the placeholder counter from `app_state`. Returns an error if the
/// singleton row is missing — it's seeded by migration `0001`, so absence
/// means the schema is broken.
pub async fn get_value(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let value = sqlx::query_scalar::<_, i64>("SELECT value FROM app_state WHERE id = 1")
        .fetch_one(pool)
        .await?;
    Ok(value)
}

/// Increment the placeholder counter atomically inside a transaction and
/// return the post-increment value.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::init_db;

    #[tokio::test]
    async fn initializes_and_seeds_default_value() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let value = get_value(&pool).await.expect("seeded value should exist");
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn increments_value_persistently() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let value = increment_value(&pool).await.unwrap();
        assert_eq!(value, 1);
        let value = get_value(&pool).await.unwrap();
        assert_eq!(value, 1);
    }
}
