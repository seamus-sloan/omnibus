//! Devices.

use sqlx::{Row, SqlitePool};

use super::{AuthResult, Device};

pub async fn register_device(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    client_kind: &str,
    client_version: Option<&str>,
) -> AuthResult<Device> {
    let row = sqlx::query(
        "INSERT INTO devices (user_id, name, client_kind, client_version)
         VALUES (?, ?, ?, ?)
         RETURNING id, user_id, name, client_kind, client_version, created_at, last_seen_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(client_kind)
    .bind(client_version)
    .fetch_one(pool)
    .await?;
    Ok(Device {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        client_kind: row.get("client_kind"),
        client_version: row.get("client_version"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
    })
}

pub async fn list_devices_for_user(pool: &SqlitePool, user_id: i64) -> AuthResult<Vec<Device>> {
    let rows = sqlx::query(
        "SELECT id, user_id, name, client_kind, client_version, created_at, last_seen_at
         FROM devices WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Device {
            id: row.get("id"),
            user_id: row.get("user_id"),
            name: row.get("name"),
            client_kind: row.get("client_kind"),
            client_version: row.get("client_version"),
            created_at: row.get("created_at"),
            last_seen_at: row.get("last_seen_at"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::pool;
    use crate::auth::users::create_user;

    #[tokio::test]
    async fn device_register_and_list() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let d = register_device(&p, u.id, "Phone", "ios", Some("1.0.0"))
            .await
            .unwrap();
        let list = list_devices_for_user(&p, u.id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, d.id);
        assert_eq!(list[0].client_kind, "ios");
    }
}
