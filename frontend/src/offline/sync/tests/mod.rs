//! Connectivity-state, error-classification and outbox-drain tests, split
//! by sub-topic into the sibling modules below; the loopback mock server
//! and queue fixtures they share live here. Real `reqwest` errors are
//! produced against live sockets — a refused connect for the offline
//! class, a garbage-body response for the decode class — so the classifier
//! is exercised on the exact error values production sees.

#![allow(clippy::await_holding_lock)]

mod classify;
mod drain;
mod replay;

use super::*;

use omnibus_shared::{ProgressRecord, ProgressUpdate};

async fn clear_ops() {
    let st = store::store().expect("test store");
    let ids: Vec<i64> = st.ops_list().await.into_iter().map(|o| o.id).collect();
    st.ops_delete_many(ids).await;
}

fn enqueue_raw(op: &Op) {
    let st = store::store().expect("test store");
    st.ops_append(
        op.kind(),
        serde_json::to_string(op).expect("payload"),
        op.coalesce_key(),
    );
}

/// Mock server covering the drain-test routes; returns its base URL.
async fn mock_server() -> String {
    use axum::routing::{delete, get, patch, post, put};
    let progress_record = |update: ProgressUpdate| ProgressRecord {
        book_uuid: update.book_uuid,
        format: update.format,
        epub_cfi: update.epub_cfi,
        audio_position_seconds: update.audio_position_seconds,
        progress_percent: update.progress_percent,
        kobo_location: update.kobo_location,
        book_file_id: None,
        updated_at: 4242,
        client_updated_at: update.client_updated_at.unwrap_or(4242),
    };
    let app = axum::Router::new()
        .route(
            "/api/progress/{uuid}",
            get(|| async { axum::Json(None::<ProgressRecord>) }),
        )
        .route(
            "/api/progress",
            post(
                move |axum::Json(update): axum::Json<ProgressUpdate>| async move {
                    axum::Json(progress_record(update))
                },
            ),
        )
        .route(
            "/api/progress/sessions",
            post(
                |axum::Json(reports): axum::Json<Vec<omnibus_shared::SessionReport>>| async move {
                    axum::Json(serde_json::json!({ "recorded": reports.len() }))
                },
            ),
        )
        .route(
            "/api/audiobooks/{uuid}/playback-rate",
            put(
                |axum::extract::Path(uuid): axum::extract::Path<String>,
                 axum::Json(update): axum::Json<omnibus_shared::AudiobookPlaybackRateUpdate>| async move {
                    axum::Json(omnibus_shared::AudiobookPlaybackRateRecord {
                        book_uuid: uuid,
                        playback_rate: update.playback_rate,
                        updated_at: 333,
                    })
                },
            ),
        )
        .route(
            "/api/ratings/{uuid}",
            delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateHighlight>| async move {
                    axum::Json(omnibus_shared::Highlight {
                        id: 55,
                        book_uuid: input.book_uuid,
                        epub_cfi_range: Some(input.epub_cfi_range),
                        color: input.color,
                        note: None,
                        text: input.text,
                        client_id: input.client_id,
                        created_at: 222,
                    })
                },
            ),
        )
        .route(
            "/api/highlights/{id}/color",
            patch(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights/{id}/note",
            patch(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights/{id}",
            delete(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route(
            "/api/bookmarks",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateBookmark>| async move {
                    axum::Json(omnibus_shared::Bookmark {
                        id: 55,
                        book_uuid: input.book_uuid,
                        position: input.position,
                        title: input.title,
                        client_id: input.client_id,
                        created_at: 111,
                    })
                },
            ),
        )
        .route(
            "/api/bookmarks/{id}",
            put(|| async { axum::http::StatusCode::OK }).delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/journals",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateJournalEntry>| async move {
                    axum::Json(omnibus_shared::JournalEntry {
                        id: 55,
                        book_uuid: input.book_uuid,
                        author_id: 1,
                        author_name: "elena".into(),
                        author_has_avatar: false,
                        body_html: format!("<p>{}</p>", input.body_md),
                        body_md: input.body_md,
                        progress: input.progress,
                        status: input.status,
                        client_id: input.client_id,
                        created_at: 444,
                        updated_at: 444,
                    })
                },
            ),
        )
        .route(
            "/api/journals/{id}",
            patch(
                |axum::extract::Path(id): axum::extract::Path<i64>,
                 axum::Json(input): axum::Json<omnibus_shared::UpdateJournalEntry>| async move {
                    axum::Json(omnibus_shared::JournalEntry {
                        id,
                        book_uuid: "journal-book".into(),
                        author_id: 1,
                        author_name: "elena".into(),
                        author_has_avatar: false,
                        body_html: format!("<p>{}</p>", input.body_md),
                        body_md: input.body_md,
                        progress: input.progress,
                        status: input.status.unwrap_or_default(),
                        client_id: None,
                        created_at: 444,
                        updated_at: 555,
                    })
                },
            )
            .delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/account/kindle-email",
            post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    // Preserve the 5xx/retry-budget behavior earlier tests assert for this address.
                    if body.get("email").and_then(|v| v.as_str()) == Some("reader@example.com") {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                },
            ),
        )
        .route(
            "/api/ratings",
            post(|| async { axum::http::StatusCode::BAD_REQUEST }),
        )
        .route(
            "/api/shelves",
            post(
                |axum::Json(req): axum::Json<omnibus_shared::CreateShelfRequest>| async move {
                    axum::Json(omnibus_shared::Shelf {
                        id: 55,
                        owner_user_id: 1,
                        owner_username: "elena".into(),
                        kind: req.kind,
                        name: req.name,
                        description: req.description,
                        visibility: req.visibility,
                        accent: None,
                        match_mode: req.match_mode,
                        rules: req.rules,
                        book_count: 0,
                        sync_to_kobo: false,
                    })
                },
            ),
        )
        .route(
            "/api/shelves/{id}",
            patch(
                |axum::extract::Path(id): axum::extract::Path<i64>,
                 axum::Json(req): axum::Json<omnibus_shared::UpdateShelfRequest>| async move {
                    axum::Json(omnibus_shared::Shelf {
                        id,
                        owner_user_id: 1,
                        owner_username: "elena".into(),
                        kind: omnibus_shared::ShelfKind::Manual,
                        name: req.name.unwrap_or_else(|| "Shelf".into()),
                        description: req.description,
                        visibility: req.visibility.unwrap_or(omnibus_shared::Visibility::Private),
                        accent: None,
                        match_mode: req.match_mode,
                        rules: req.rules.unwrap_or_default(),
                        book_count: 0,
                        sync_to_kobo: req.sync_to_kobo.unwrap_or(false),
                    })
                },
            )
            .delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/shelves/{id}/books",
            post(
                |axum::extract::Path(id): axum::extract::Path<i64>| async move {
                    // The remapped id must be the server-assigned one.
                    if id == 55 {
                        axum::http::StatusCode::OK
                    } else {
                        axum::http::StatusCode::NOT_FOUND
                    }
                },
            ),
        )
        .route(
            "/api/shelves/{id}/books/{uuid}",
            delete(|| async { axum::http::StatusCode::OK }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}
