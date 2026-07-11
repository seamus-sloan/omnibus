//! F4.3 Send-to-Kindle: email a book's EPUB as an attachment over SMTP.
//!
//! Called from the background worker (`Task::SendToKindle`). Resolves the EPUB
//! bytes via the same on-disk path the download route uses, builds a MIME
//! message with the file attached as `application/epub+zip`, and delivers it
//! through the admin-configured SMTP relay. No format conversion — Amazon
//! accepts `.epub` directly.

use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sqlx::SqlitePool;

use crate::settings::{SmtpConfig, SmtpSecurity};

/// lettre's built-in timeout is *per SMTP command*, so a slow or hung relay can
/// blow far past it across connect → EHLO → AUTH → DATA. This caps the whole
/// delivery so the worker task — and the UI awaiting it — can never hang
/// indefinitely.
const SEND_TIMEOUT: Duration = Duration::from_secs(90);

/// Per-command network timeout handed to lettre, so a single stalled command
/// fails fast well inside [`SEND_TIMEOUT`] instead of leaning on the overall cap.
const PER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Failure modes of the send path. Callers branch on these to surface a
/// per-case message (`NotConfigured` / `NoEpub` render distinct UI hints),
/// while transport/build failures collapse into the generic delivery errors.
#[derive(Debug, thiserror::Error)]
pub enum KindleError {
    #[error("email delivery is not configured on this server")]
    NotConfigured,
    #[error("this book has no EPUB file to send")]
    NoEpub,
    #[error(
        "this EPUB is larger than the 50 MB limit for emailing files to a Kindle; upload it on \
         Amazon's Send to Kindle page (amazon.com/sendtokindle), which accepts files up to 200 MB"
    )]
    TooLarge,
    #[error("failed to read the EPUB file: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid email address: {0}")]
    Address(#[from] lettre::address::AddressError),
    #[error("failed to build the email: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP delivery failed: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),
    #[error("SMTP delivery timed out after {}s", SEND_TIMEOUT.as_secs())]
    Timeout,
    #[error(transparent)]
    Settings(#[from] crate::settings::SettingsError),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
}

/// Send the EPUB for `book_id` (optionally a specific `book_file_id` for
/// multi-EPUB books) to `to_email` via the effective SMTP config. Errors with
/// [`KindleError::NotConfigured`] when no SMTP relay is set and
/// [`KindleError::NoEpub`] when the book has no EPUB on disk.
pub async fn send(
    pool: &SqlitePool,
    book_id: i64,
    book_file_id: Option<i64>,
    to_email: &str,
) -> Result<(), KindleError> {
    let config = crate::settings::effective_smtp_config(pool)
        .await?
        .ok_or(KindleError::NotConfigured)?;

    let path = match book_file_id {
        Some(fid) => crate::book_file_path_by_id(pool, book_id, fid, Some("EPUB")).await?,
        None => crate::book_file_path(pool, book_id, "EPUB").await?,
    }
    .ok_or(KindleError::NoEpub)?;

    let bytes = tokio::fs::read(&path).await?;
    // Defense in depth: the UI disables the button for oversized EPUBs, but the
    // REST/RPC endpoints (and mobile) can still enqueue a send. Reject here so a
    // doomed 150 MB delivery never reaches the relay — surfaced to the poller as
    // a clear Failed message via `TooLarge`'s display text.
    if omnibus_shared::kindle_email_oversize(bytes.len() as u64) {
        return Err(KindleError::TooLarge);
    }
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("book.epub")
        .to_string();

    let email = build_epub_email(&config.from_email, to_email, &filename, bytes)?;
    deliver(&build_transport(&config)?, email).await
}

/// Send one already-built message under the overall [`SEND_TIMEOUT`] cap. Maps
/// the elapsed case to [`KindleError::Timeout`] so a hung relay surfaces a
/// clear, bounded failure instead of leaving the caller awaiting forever.
async fn deliver(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    email: Message,
) -> Result<(), KindleError> {
    match tokio::time::timeout(SEND_TIMEOUT, transport.send(email)).await {
        Ok(res) => res.map(|_| ()).map_err(KindleError::from),
        Err(_elapsed) => Err(KindleError::Timeout),
    }
}

/// Send a small no-attachment message to `to_email` so an admin can verify the
/// SMTP config works before users rely on it. Same transport as [`send`].
pub async fn send_test(pool: &SqlitePool, to_email: &str) -> Result<(), KindleError> {
    let config = crate::settings::effective_smtp_config(pool)
        .await?
        .ok_or(KindleError::NotConfigured)?;
    let email = Message::builder()
        .from(config.from_email.parse()?)
        .to(to_email.parse()?)
        .subject("Omnibus SMTP test")
        .header(ContentType::TEXT_PLAIN)
        .body("This is a test email from Omnibus. Your SMTP settings work.".to_string())?;
    deliver(&build_transport(&config)?, email).await
}

/// Build the MIME message: a short plain-text body plus the EPUB attached as
/// `application/epub+zip`. Pure (no IO) so the message shape is unit-testable.
fn build_epub_email(
    from: &str,
    to: &str,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<Message, KindleError> {
    let attachment = Attachment::new(filename.to_string()).body(
        bytes,
        ContentType::parse("application/epub+zip").expect("static content type parses"),
    );
    let message = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject(filename.trim_end_matches(".epub").to_string())
        .multipart(
            MultiPart::mixed()
                .singlepart(SinglePart::plain(
                    "Sent from your Omnibus library.".to_string(),
                ))
                .singlepart(attachment),
        )?;
    Ok(message)
}

/// Build the async SMTP transport for `config`, selecting implicit TLS or
/// STARTTLS per [`SmtpSecurity`] and attaching credentials only when a
/// username is set (unauthenticated relays leave it blank).
fn build_transport(config: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, KindleError> {
    let builder = match config.security {
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)?,
        SmtpSecurity::Starttls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
        }
    }
    .port(config.port)
    .timeout(Some(PER_COMMAND_TIMEOUT))
    // lettre defaults the EHLO name to the OS hostname, which on some machines
    // (e.g. a Mac named "Ethan Wilkes …") contains spaces and is rejected by
    // strict relays like Gmail with a 501 syntax error. Pin a valid name.
    .hello_name(ClientId::Domain("localhost".to_string()));
    let builder = if config.username.is_empty() {
        builder
    } else {
        builder.credentials(Credentials::new(
            config.username.clone(),
            config.password.clone(),
        ))
    };
    Ok(builder.build())
}

#[cfg(test)]
mod tests;
