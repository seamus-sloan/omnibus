//! The content validator (HTTP `ETag`) for a file served off disk.
//!
//! One recipe, shared by the db layer that reports it and the server handlers
//! that stamp it, because a client compares the two — so the whole contract is
//! that both sides produce byte-identical strings.

/// The `ETag` for a file with this filesystem stat, quotes included.
///
/// `(mtime, size)` is the nginx/Apache default-ETag recipe: one `stat`
/// instead of a read of the whole file, which is what makes it affordable on
/// an audiobook library where hashing content per request would dominate the
/// response. The cost is a blind spot — a replacement landing in the same
/// whole second *and* keeping the byte length identical is invisible. That is
/// the same blind spot the indexer already has, since `(mtime_epoch,
/// size_bytes)` is its own change-detection key, so a file this cannot see
/// change is one the library would not have re-read either.
///
/// Emitted as a **strong** validator, matching nginx, despite that blind
/// spot: `If-Range` is defined only under strong comparison, so a `W/` prefix
/// would make every resumed download restart from byte zero.
///
/// `None` for the `(0, 0)` sentinel — a `book_files` row whose stat has never
/// been observed (pre-`0009` rows awaiting the backfill). Reporting no
/// validator is honest; reporting `"0-0"` would collide across every such
/// row.
pub fn content_validator(mtime_epoch: i64, size_bytes: i64) -> Option<String> {
    if mtime_epoch == 0 && size_bytes == 0 {
        return None;
    }
    Some(format!("\"{mtime_epoch:x}-{size_bytes:x}\""))
}

#[cfg(test)]
mod tests;
