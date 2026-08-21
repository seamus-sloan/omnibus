use super::send_to_kindle;
use omnibus_db::test_support::seed_synced_ebook;
use omnibus_db::worker::{Worker, WorkerConfig};
use omnibus_shared::{SmtpConfigUpdate, SmtpSecurity};

async fn pool_with_user() -> (sqlx::SqlitePool, i64) {
    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    let user_id = omnibus_db::auth::create_user(&pool, "reader", "securepassword1")
        .await
        .unwrap()
        .id;
    (pool, user_id)
}

async fn configure_smtp(pool: &sqlx::SqlitePool) {
    omnibus_db::set_smtp_config(
        pool,
        &SmtpConfigUpdate {
            host: "smtp.example.com".into(),
            port: 587,
            username: "relay".into(),
            from_email: "library@example.com".into(),
            security: SmtpSecurity::Starttls,
            password: Some("secret".into()),
        },
    )
    .await
    .unwrap();
}

fn worker(pool: sqlx::SqlitePool) -> std::sync::Arc<Worker> {
    Worker::new(pool, WorkerConfig::default())
}

#[tokio::test]
async fn send_to_kindle_rejects_an_oversized_book_uuid() {
    let (pool, user_id) = pool_with_user().await;
    let w = worker(pool.clone());
    let oversized = "x".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1);

    let err = send_to_kindle(&pool, &w, user_id, &oversized, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("book_uuid must be"));
}

#[tokio::test]
async fn send_to_kindle_requires_a_kindle_email_on_the_account() {
    let (pool, user_id) = pool_with_user().await;
    configure_smtp(&pool).await;
    let w = worker(pool.clone());
    let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;

    let err = send_to_kindle(&pool, &w, user_id, &uuid, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Kindle email"));
}

#[tokio::test]
async fn send_to_kindle_requires_smtp_to_be_configured() {
    let (pool, user_id) = pool_with_user().await;
    omnibus_db::auth::set_kindle_email(&pool, user_id, Some("reader@kindle.com"))
        .await
        .unwrap();
    let w = worker(pool.clone());
    let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;

    let err = send_to_kindle(&pool, &w, user_id, &uuid, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not configured"));
}

#[tokio::test]
async fn send_to_kindle_reports_book_not_found_for_an_unknown_uuid() {
    let (pool, user_id) = pool_with_user().await;
    omnibus_db::auth::set_kindle_email(&pool, user_id, Some("reader@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;
    let w = worker(pool.clone());

    let err = send_to_kindle(&pool, &w, user_id, "no-such-uuid", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("book not found"));
}

#[tokio::test]
async fn send_to_kindle_enqueues_a_task_when_every_precondition_passes() {
    let (pool, user_id) = pool_with_user().await;
    omnibus_db::auth::set_kindle_email(&pool, user_id, Some("reader@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;
    let w = worker(pool.clone());
    let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;

    let task_id = send_to_kindle(&pool, &w, user_id, &uuid, None)
        .await
        .unwrap();
    assert!(task_id > 0);
}

#[tokio::test]
async fn send_to_kindle_task_owner_scoping_hides_status_from_another_user() {
    let (pool, owner_id) = pool_with_user().await;
    // Plain `create_user` (self-registration) refuses a second account
    // unless `registration_enabled` is set; `admin_create_user` bypasses
    // that gate.
    let other_id = omnibus_db::auth::admin_create_user(
        &pool,
        "other",
        "securepassword1",
        omnibus_shared::UserPermissions {
            is_admin: false,
            can_upload: false,
            can_edit: false,
            can_download: true,
        },
    )
    .await
    .unwrap()
    .id;
    omnibus_db::auth::set_kindle_email(&pool, owner_id, Some("reader@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;
    let w = worker(pool.clone());
    let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;

    let task_id = send_to_kindle(&pool, &w, owner_id, &uuid, None)
        .await
        .unwrap();

    assert!(
        w.owned_task_state(task_id, owner_id).is_some(),
        "the owning user must see the task's status"
    );
    assert!(
        w.owned_task_state(task_id, other_id).is_none(),
        "a different user must not be able to poll another user's send"
    );
}
