//! Send-to-Kindle server functions. `rpc_send_to_kindle` enqueues a
//! `SendToKindle` job on the shared worker and returns its `task_id`
//! immediately; the client then polls `rpc_kindle_send_status` for the terminal
//! result. Returning right away keeps the request well under the server's 30s
//! timeout guard, so a hung SMTP relay never leaves the request open.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::KindleSendStatus;
#[cfg(feature = "server")]
use omnibus_shared::ProgressState;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt, WorkerExt};

/// Enqueue a send of the EPUB for `book_uuid` (optionally a specific `file_id`
/// for multi-EPUB books) to the authenticated user's Kindle address, returning
/// the worker `task_id` to poll. Errors synchronously when the user has no
/// Kindle email, when SMTP is unconfigured, or when the book can't be resolved
/// — those are fast checks the button surfaces immediately. The actual SMTP
/// delivery runs on the worker; poll [`rpc_kindle_send_status`] for its result.
#[post("/api/rpc/kindle/send", pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_send_to_kindle(book_uuid: String, file_id: Option<i64>) -> Result<u64> {
    Ok(send_to_kindle(&pool.0, &worker.0, user.id, &book_uuid, file_id).await?)
}

/// Server-side body of [`rpc_send_to_kindle`], extracted so the four-step
/// precondition chain (uuid length → Kindle email set → SMTP configured →
/// book resolves) and the owner-scoping enqueue can be unit-tested without
/// the server-fn transport.
#[cfg(feature = "server")]
async fn send_to_kindle(
    pool: &sqlx::SqlitePool,
    worker: &std::sync::Arc<db::worker::Worker>,
    user_id: i64,
    book_uuid: &str,
    file_id: Option<i64>,
) -> Result<u64, ServerFnError> {
    if book_uuid.len() > omnibus_shared::BOOK_UUID_MAX_LEN {
        return Err(ServerFnError::new(format!(
            "book_uuid must be ≤ {} bytes",
            omnibus_shared::BOOK_UUID_MAX_LEN
        )));
    }
    let recipient = db::auth::get_kindle_email(pool, user_id)
        .await
        .map_err(|e| internal_rpc_error("get kindle email", e))?;
    let Some(recipient) = recipient else {
        return Err(ServerFnError::new(
            "add your Kindle email on your account page before sending",
        ));
    };
    let smtp_configured = db::effective_smtp_config(pool)
        .await
        .map_err(|e| internal_rpc_error("get smtp config", e))?
        .is_some();
    if !smtp_configured {
        return Err(ServerFnError::new(
            "email delivery is not configured on this server",
        ));
    }
    let Some(book_id) = db::resolve_book_id_by_uuid(pool, book_uuid)
        .await
        .map_err(|e| internal_rpc_error("resolve book id", e))?
    else {
        return Err(ServerFnError::new("book not found"));
    };

    let id = worker.post(omnibus_db::worker::Task::SendToKindle {
        book_id,
        book_file_id: file_id,
        recipient_email: recipient,
    });
    // Scope the pollable status to this user so the guessable task-id space
    // can't be probed for other users' send outcomes.
    worker.set_task_owner(id, user_id);
    Ok(id)
}

/// Poll the status of a send enqueued by [`rpc_send_to_kindle`]. Returns
/// `None` once `task_id` is unknown — never posted, evicted after the worker's
/// terminal-retention window (~10s past completion), or not owned by the
/// caller. Scoped to the requesting user so the guessable task-id space can't
/// be probed for other users' send outcomes. The client polls this until it
/// observes a non-`Pending` status.
#[post("/api/rpc/kindle/send/status", _pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_kindle_send_status(task_id: u64) -> Result<Option<KindleSendStatus>> {
    Ok(worker
        .0
        .owned_task_state(task_id, user.id)
        .map(|state| match state {
            ProgressState::Running { .. } => KindleSendStatus::Pending,
            ProgressState::Done { .. } => KindleSendStatus::Sent,
            ProgressState::Failed { message } => KindleSendStatus::Failed { message },
        }))
}

// `server`-gated: exercises the precondition chain and the task-owner
// scoping directly against an in-memory DB + a real `Worker`, mirroring
// `overrides.rs`'s pattern. CI runs this via `cargo test -p omnibus-frontend
// --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
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
}
