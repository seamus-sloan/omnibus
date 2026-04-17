use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

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
}
