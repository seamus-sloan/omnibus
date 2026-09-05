//! Tests for the Physical Check-In scan REST handlers, split by route into
//! the sibling modules below; the fake user, seeded-book and request
//! fixtures they share live here. These stay network-free: an exact-ISBN
//! hit and an invalid ISBN both resolve before any provider call, and the
//! write paths never touch the network — the online rungs are covered by
//! the `omnibus_db::scan` wiremock tests.

mod google_books_key;
mod resolve;
mod writes;

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request},
};

use crate::backend::test_support::*;

const ISBN: &str = "9780134685991";

/// A well-formed `ExternalBookMeta` JSON body, as `AddPhysicalOnlyRequest` and
/// `WishlistAddRequest` embed it.
fn external_meta_json(title: &str, isbn13: &str) -> serde_json::Value {
    serde_json::json!({
        "isbn13": isbn13,
        "title": title,
        "authors": ["Jane Doe"],
        "year": null,
        "pages": null,
        "publisher": null,
        "description": null,
        "cover_url": null,
        "source": "open_library",
    })
}

fn post(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn seed_book_with_isbn(pool: &sqlx::SqlitePool, title: &str, isbn: &str) -> String {
    let (id, uuid) = seed_book_with_uuid(pool, "/lib", title).await;
    sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?1, 'ISBN', ?2)")
        .bind(id)
        .bind(isbn)
        .execute(pool)
        .await
        .unwrap();
    uuid
}

async fn json_body<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}
